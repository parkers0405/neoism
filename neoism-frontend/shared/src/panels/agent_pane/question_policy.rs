//! Pending model-question state shared by native and wasm panes.
//!
//! The agent server's `question` tool parks the run on a oneshot until
//! the user answers (`POST /question/{id}/reply` with
//! `answers: Vec<Vec<String>>`, one list per question in the request).
//! This module owns the pure state machine the prompt picker drives:
//! option filtering, the free-typed answer row, sequential multi-question
//! answering, and the queue policy (reused from `permission_policy`).

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentQuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentQuestionItem {
    pub text: String,
    pub options: Vec<NeoismAgentQuestionOption>,
}

/// One row of the question prompt picker: a filtered option, or the
/// synthetic "Answer: <typed>" row that commits the free-typed text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionRow {
    pub label: String,
    pub description: String,
    pub is_custom: bool,
}

/// Outcome of committing the selected row for the current question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionCommit {
    /// Nothing selectable (e.g. free-text question with nothing typed).
    Nothing,
    /// Answer recorded; a later question in the same request is now
    /// current — keep the prompt open.
    Advanced,
    /// Every question answered — send this to `/question/{id}/reply`.
    Finished(Vec<Vec<String>>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentPendingQuestion {
    pub id: String,
    pub session_id: String,
    pub questions: Vec<NeoismAgentQuestionItem>,
    /// Index of the question currently shown — multi-question requests
    /// are answered sequentially and sent as one reply.
    pub current: usize,
    /// Answers committed for questions before `current`.
    pub answers: Vec<Vec<String>>,
    /// Keyboard-selected row within `visible_rows()`.
    pub selected: usize,
    /// Free-typed buffer shown in the picker's search row. Filters the
    /// options and doubles as the custom answer.
    pub typed: String,
    pub responding: bool,
}

impl NeoismAgentPendingQuestion {
    pub fn current_item(&self) -> Option<&NeoismAgentQuestionItem> {
        self.questions.get(self.current)
    }

    /// Rows the prompt picker shows for the current question: options
    /// matching the typed filter, plus a trailing custom-answer row
    /// whenever something is typed (so a free answer is always one
    /// Enter away, even when options exist).
    pub fn visible_rows(&self) -> Vec<QuestionRow> {
        let Some(item) = self.current_item() else {
            return Vec::new();
        };
        let needle = self.typed.trim().to_lowercase();
        let mut rows: Vec<QuestionRow> = item
            .options
            .iter()
            .filter(|option| {
                needle.is_empty()
                    || option.label.to_lowercase().contains(&needle)
                    || option.description.to_lowercase().contains(&needle)
            })
            .map(|option| QuestionRow {
                label: option.label.clone(),
                description: option.description.clone(),
                is_custom: false,
            })
            .collect();
        if !self.typed.trim().is_empty() {
            rows.push(QuestionRow {
                label: format!("Answer: {}", self.typed.trim()),
                description: "Send your typed answer".to_string(),
                is_custom: true,
            });
        }
        rows
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.visible_rows().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let current = self.selected.min(count - 1) as isize;
        self.selected = (current + delta).rem_euclid(count as isize) as usize;
    }

    pub fn type_str(&mut self, text: &str) {
        self.typed.push_str(text);
        self.selected = 0;
    }

    pub fn backspace(&mut self) {
        self.typed.pop();
        self.selected = 0;
    }

    /// Commit the selected row (or the bare typed text for option-less
    /// questions) as the current question's answer.
    pub fn commit_selected(&mut self) -> QuestionCommit {
        let rows = self.visible_rows();
        let answer = match rows.get(self.selected.min(rows.len().saturating_sub(1))) {
            Some(row) if row.is_custom => self.typed.trim().to_string(),
            Some(row) => row.label.clone(),
            None => {
                let typed = self.typed.trim();
                if typed.is_empty() {
                    return QuestionCommit::Nothing;
                }
                typed.to_string()
            }
        };
        self.answers.push(vec![answer]);
        self.current += 1;
        self.typed.clear();
        self.selected = 0;
        if self.current < self.questions.len() {
            QuestionCommit::Advanced
        } else {
            QuestionCommit::Finished(self.answers.clone())
        }
    }
}

fn question_option_from_value(value: &Value) -> Option<NeoismAgentQuestionOption> {
    if let Some(label) = value.as_str() {
        let label = label.trim();
        return (!label.is_empty()).then(|| NeoismAgentQuestionOption {
            label: label.to_string(),
            description: String::new(),
        });
    }
    let label = value
        .get("label")
        .or_else(|| value.get("title"))
        .or_else(|| value.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if label.is_empty() {
        return None;
    }
    Some(NeoismAgentQuestionOption {
        label,
        description: value
            .get("description")
            .or_else(|| value.get("hint"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn question_item_from_value(value: &Value) -> NeoismAgentQuestionItem {
    NeoismAgentQuestionItem {
        text: value
            .get("question")
            .or_else(|| value.get("label"))
            .or_else(|| value.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("The agent has a question")
            .to_string(),
        options: value
            .get("options")
            .or_else(|| value.get("choices"))
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(question_option_from_value)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// Parse a `question.asked` event payload (a serialized
/// `QuestionRequestInfo`) into pending-question state. Lenient by
/// design — the tool schema lets the model send loose objects.
pub fn question_request_from_event(properties: &Value) -> NeoismAgentPendingQuestion {
    NeoismAgentPendingQuestion {
        id: properties
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        session_id: properties
            .get("sessionID")
            .or_else(|| properties.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        questions: properties
            .get("questions")
            .and_then(Value::as_array)
            .map(|questions| questions.iter().map(question_item_from_value).collect())
            .unwrap_or_default(),
        current: 0,
        answers: Vec::new(),
        selected: 0,
        typed: String::new(),
        responding: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn question(options: &[&str]) -> NeoismAgentPendingQuestion {
        NeoismAgentPendingQuestion {
            id: "q-1".into(),
            session_id: "s-1".into(),
            questions: vec![NeoismAgentQuestionItem {
                text: "Pick one".into(),
                options: options
                    .iter()
                    .map(|label| NeoismAgentQuestionOption {
                        label: (*label).to_string(),
                        description: String::new(),
                    })
                    .collect(),
            }],
            current: 0,
            answers: Vec::new(),
            selected: 0,
            typed: String::new(),
            responding: false,
        }
    }

    #[test]
    fn parses_loose_question_payload() {
        let pending = question_request_from_event(&json!({
            "id": "que_1",
            "sessionID": "ses_1",
            "questions": [
                { "question": "Deploy now?", "options": ["Yes", { "label": "No", "description": "wait" }] },
                { "label": "Which env?" }
            ]
        }));
        assert_eq!(pending.id, "que_1");
        assert_eq!(pending.questions.len(), 2);
        assert_eq!(pending.questions[0].options.len(), 2);
        assert_eq!(pending.questions[0].options[1].description, "wait");
        assert!(pending.questions[1].options.is_empty());
    }

    #[test]
    fn typed_text_filters_and_adds_custom_row() {
        let mut pending = question(&["Alpha", "Beta"]);
        pending.type_str("al");
        let rows = pending.visible_rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Alpha");
        assert!(rows[1].is_custom);
    }

    #[test]
    fn commit_option_finishes_single_question() {
        let mut pending = question(&["Alpha", "Beta"]);
        pending.move_selection(1);
        assert_eq!(
            pending.commit_selected(),
            QuestionCommit::Finished(vec![vec!["Beta".to_string()]])
        );
    }

    #[test]
    fn free_text_question_requires_typed_answer() {
        let mut pending = question(&[]);
        assert_eq!(pending.commit_selected(), QuestionCommit::Nothing);
        pending.type_str("ship it");
        assert_eq!(
            pending.commit_selected(),
            QuestionCommit::Finished(vec![vec!["ship it".to_string()]])
        );
    }

    #[test]
    fn multi_question_advances_then_finishes() {
        let mut pending = question(&["Alpha"]);
        pending.questions.push(NeoismAgentQuestionItem {
            text: "Second".into(),
            options: vec![],
        });
        assert_eq!(pending.commit_selected(), QuestionCommit::Advanced);
        pending.type_str("done");
        assert_eq!(
            pending.commit_selected(),
            QuestionCommit::Finished(vec![
                vec!["Alpha".to_string()],
                vec!["done".to_string()]
            ])
        );
    }
}
