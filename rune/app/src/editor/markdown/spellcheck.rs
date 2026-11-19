//! Native spelling, independent of Rune's platform spell checker (a Linux noop).
//! Dictionary loading, tokenization and suggestions run on one bounded worker.

#[path = "../../../../../neoism-frontend/shared/src/editor/markdown/spellcheck.rs"]
mod dictionary;

use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock, mpsc},
};

const CACHE_LIMIT: usize = 256;
const LOCAL_LIMIT: usize = 128;
const QUEUE_LIMIT: usize = 32;
// Pathological single-line documents must not monopolize the worker or cache.
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_WORD_BYTES: usize = 64;

#[derive(Clone, Debug)]
pub(super) struct Word {
    /// UTF-8 byte offsets into the exact string supplied to `check`.
    pub range: Range<usize>,
    pub text: String,
    pub suggestions: Vec<String>,
}

#[derive(Default)]
pub(super) struct State {
    local: RefCell<Local>,
}

#[derive(Default)]
struct Local {
    generation: u64,
    ready: HashMap<String, Rc<Vec<Word>>>,
}

enum Entry {
    Pending,
    Ready(Arc<Vec<Word>>),
}

#[derive(Default)]
struct Shared {
    generation: u64,
    epoch: u64,
    entries: HashMap<String, Entry>,
    // Rune uses one window waker; keep the latest subscription, including when
    // the queue is full, so a completion retries requests not yet admitted.
    wake: Option<rune::Waker>,
}

struct Request {
    text: String,
    generation: u64,
}

struct Service {
    shared: Arc<Mutex<Shared>>,
    send: mpsc::SyncSender<Request>,
}

fn service() -> &'static Service {
    static SERVICE: OnceLock<Service> = OnceLock::new();
    SERVICE.get_or_init(|| {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (send, receive) = mpsc::sync_channel::<Request>(QUEUE_LIMIT);
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("markdown-spelling".into())
            .spawn(move || {
                while let Ok(request) = receive.recv() {
                    // Ignore work invalidated before it even left the queue.
                    let stale_wake = {
                        let state = worker.lock().unwrap();
                        (state.generation != request.generation)
                            .then(|| state.wake.clone())
                    };
                    if let Some(wake) = stale_wake {
                        // Draining obsolete work frees queue capacity too. A
                        // rejected new-generation request must get a retry.
                        if let Some(wake) = wake {
                            wake();
                        }
                        continue;
                    }
                    let words = Arc::new(check_words(&request.text));
                    let wake = {
                        let mut state = worker.lock().unwrap();
                        if state.generation == request.generation {
                            state.entries.insert(request.text, Entry::Ready(words));
                            state.epoch = state.epoch.wrapping_add(1);
                        }
                        state.wake.clone()
                    };
                    // Never call user code with the cache lock held.
                    if let Some(wake) = wake {
                        wake();
                    }
                }
            })
            .expect("start Markdown spelling worker");
        Service { shared, send }
    })
}

impl State {
    /// `None` means queued (or temporarily full); completion wakes the window.
    /// Inputs over 16 KiB are deliberately skipped, returning an empty result.
    pub(super) fn check(
        &self,
        text: &str,
        waker: Option<rune::Waker>,
    ) -> Option<Rc<Vec<Word>>> {
        if text.len() > MAX_TEXT_BYTES {
            return Some(Rc::new(Vec::new()));
        }
        let service = service();
        let mut shared = service.shared.lock().unwrap();
        if waker.is_some() {
            shared.wake = waker;
        }
        let mut local = self.local.borrow_mut();
        if local.generation != shared.generation {
            local.generation = shared.generation;
            local.ready.clear();
        }
        if let Some(words) = local.ready.get(text) {
            return Some(words.clone());
        }
        match shared.entries.get(text) {
            Some(Entry::Ready(words)) => {
                let words = Rc::new((**words).clone());
                if local.ready.len() == LOCAL_LIMIT {
                    local.ready.clear();
                }
                local.ready.insert(text.to_owned(), words.clone());
                return Some(words);
            }
            Some(Entry::Pending) => return None,
            None => {}
        }
        if shared.entries.len() == CACHE_LIMIT {
            // Never evict pending work: repeated views must deduplicate it.
            let victim = shared.entries.iter().find_map(|(text, entry)| {
                matches!(entry, Entry::Ready(_)).then(|| text.clone())
            });
            if let Some(victim) = victim {
                shared.entries.remove(&victim);
            } else {
                return None;
            }
        }
        let request = Request {
            text: text.to_owned(),
            generation: shared.generation,
        };
        if service.send.try_send(request).is_ok() {
            shared.entries.insert(text.to_owned(), Entry::Pending);
        }
        None
    }

    /// Global across states, including worker completions and override changes.
    pub(super) fn epoch(&self) -> u64 {
        service().shared.lock().unwrap().epoch
    }

    pub(super) fn ignore(&self, word: &str) {
        dictionary::ignore_spelling_word(word);
        invalidate();
    }

    /// Persists to Neoism's global `dictionary.txt`, not a Rune-only dictionary.
    pub(super) fn learn(&self, word: &str) -> std::io::Result<()> {
        dictionary::add_spelling_word_to_dictionary(word)?;
        invalidate();
        Ok(())
    }
}

fn invalidate() {
    let wake = {
        let mut shared = service().shared.lock().unwrap();
        shared.generation = shared.generation.wrapping_add(1);
        shared.epoch = shared.epoch.wrapping_add(1);
        shared.entries.clear();
        shared.wake.clone()
    };
    if let Some(wake) = wake {
        wake();
    }
}

fn check_words(text: &str) -> Vec<Word> {
    // Repeated words only pay for dictionary suggestion search once per line.
    let mut checked = HashMap::<&str, Option<Vec<String>>>::new();
    dictionary::spellcheck_words(text)
        .into_iter()
        .filter_map(|word| {
            if word.text.len() > MAX_WORD_BYTES {
                return None;
            }
            let suggestions = checked.entry(word.text).or_insert_with(|| {
                dictionary::is_misspelled_word(word.text)
                    .then(|| dictionary::spelling_suggestions(word.text))
            });
            suggestions.as_ref().map(|suggestions| Word {
                range: word.start..word.start + word.text.len(),
                text: word.text.to_owned(),
                suggestions: suggestions.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(state: &State, text: &str) -> Rc<Vec<Word>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(words) = state.check(text, None) {
                return words;
            }
            assert!(Instant::now() < deadline, "spelling worker timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn real_worker_corrects_common_typos_and_caches_byte_ranges() {
        let state = State::default();
        let text = "“mispelled”, recieve and `teh` person@example.com cache_key64";
        let words = wait(&state, text);
        for (typo, correction) in [("mispelled", "misspelled"), ("recieve", "receive")] {
            let word = words.iter().find(|word| word.text == typo).expect(typo);
            assert_eq!(&text[word.range.clone()], typo);
            assert!(
                word.suggestions.iter().any(|word| word == correction),
                "{word:?}"
            );
        }
        assert!(!words.iter().any(|word| word.text == "teh"));
        assert!(Rc::ptr_eq(&words, &wait(&state, text)));
    }

    #[test]
    fn completion_wakes_the_window() {
        let state = State::default();
        let (send, receive) = mpsc::channel();
        let wake: rune::Waker = Arc::new(move || {
            let _ = send.send(());
        });
        let text = "a seperate completion subscription";
        let before = state.epoch();
        if state.check(text, Some(wake)).is_none() {
            receive
                .recv_timeout(Duration::from_secs(30))
                .expect("worker wake");
        }
        let words = wait(&state, text);
        assert!(words.iter().any(|word| word.text == "seperate"));
        assert!(state.epoch() > before);
    }

    #[test]
    fn ignore_invalidates_other_states_and_pending_work() {
        let first = State::default();
        let second = State::default();
        let text = "an independant spelling example";
        assert!(
            wait(&second, text)
                .iter()
                .any(|word| word.text == "independant")
        );
        let epoch = second.epoch();
        first.ignore("independant");
        assert!(second.epoch() > epoch);
        assert!(
            !wait(&second, text)
                .iter()
                .any(|word| word.text == "independant")
        );
    }

    #[test]
    fn request_and_cache_sizes_are_bounded() {
        let state = State::default();
        for index in 0..CACHE_LIMIT * 2 {
            state.check(&format!("mispelled example {index}"), None);
        }
        let shared = service().shared.lock().unwrap();
        assert!(shared.entries.len() <= CACHE_LIMIT);
        assert!(
            shared
                .entries
                .values()
                .filter(|entry| matches!(entry, Entry::Pending))
                .count()
                <= QUEUE_LIMIT + 1
        );
        drop(shared);
        assert!(
            state
                .check(&"a".repeat(MAX_TEXT_BYTES + 1), None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn native_dictionary_handles_non_curated_words_when_installed() {
        // The supported fallback is deliberately conservative on machines
        // without /usr/share/dict; never substitute a password word list.
        if dictionary::spellcheck_dictionary().is_none() {
            return;
        }
        let state = State::default();
        let words = wait(&state, "ordinary zzzzzordinary words");
        assert!(!words.iter().any(|word| word.text == "ordinary"));
        assert!(words.iter().any(|word| word.text == "zzzzzordinary"));
    }
}
