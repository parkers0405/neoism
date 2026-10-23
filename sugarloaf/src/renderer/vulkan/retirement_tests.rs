use super::SlotRetirement;
use std::cell::RefCell;
use std::rc::Rc;

struct Tracked(Rc<RefCell<Vec<&'static str>>>, &'static str);
impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.borrow_mut().push(self.1);
    }
}

// Stand-in for the whole VulkanImageTexture, not merely its VkImage handle.
struct Texture {
    _image: Tracked,
    _descriptor_pool: Tracked,
}
fn texture(log: &Rc<RefCell<Vec<&'static str>>>) -> Texture {
    Texture {
        _image: Tracked(log.clone(), "image"),
        _descriptor_pool: Tracked(log.clone(), "descriptor pool"),
    }
}

#[test]
fn actual_slot_reuse_not_elapsed_frames_releases_entire_texture() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut retirement = SlotRetirement::default();
    retirement.retire(texture(&log));
    retirement.acquired(2);
    // Ready-slot selection need not be round-robin. Arbitrarily many frames
    // on other slots cannot prove completion of slot 2's retirement frame.
    for slot in [0, 1, 0, 0, 1, 1, 0] {
        retirement.acquired(slot);
        assert!(log.borrow().is_empty());
    }
    retirement.acquired(2);
    assert_eq!(*log.borrow(), ["image", "descriptor pool"]);
}

#[test]
fn old_bin_drops_before_new_pending_enters_same_slot() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut retirement = SlotRetirement::default();
    retirement.retire(texture(&log));
    retirement.acquired(1);
    retirement.retire(texture(&log));
    retirement.acquired(1);
    assert_eq!(log.borrow().len(), 2);
    assert!(retirement.pending.is_empty());
    assert_eq!(retirement.bins[1].len(), 1);
    retirement.acquired(1);
    assert_eq!(log.borrow().len(), 4);
}

#[test]
fn failed_acquisition_does_not_advance_pending_or_any_bin() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut retirement = SlotRetirement::default();
    retirement.retire(texture(&log));
    retirement.acquired(0);
    retirement.retire(texture(&log));
    // The caller skips the hook on None; repeated failures have no transition.
    for _ in 0..8 {
        assert_eq!(retirement.pending.len(), 1);
        assert_eq!(retirement.bins[0].len(), 1);
        assert!(log.borrow().is_empty());
    }
    retirement.acquired(2);
    assert_eq!(retirement.bins[0].len(), 1);
    assert_eq!(retirement.bins[2].len(), 1);
    assert!(log.borrow().is_empty());
}

#[test]
fn completed_slot_reuse_bounds_retained_batches_during_continuous_frames() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut retirement = SlotRetirement::default();
    for frame in 0..1_000 {
        retirement.retire(texture(&log));
        retirement.acquired(frame % super::FRAMES_IN_FLIGHT);
        assert!(retirement.pending.is_empty());
        // One batch per actual slot, not a growing elapsed-frame history.
        assert!(retirement.bins.iter().all(|bin| bin.len() <= 1));
        let retained: usize = retirement.bins.iter().map(Vec::len).sum();
        assert!(retained <= super::FRAMES_IN_FLIGHT);
        assert_eq!(log.borrow().len() / 2 + retained, frame + 1);
    }
    retirement.clear_after_idle();
    assert_eq!(log.borrow().len(), 2_000);
}

#[test]
fn acquired_retirement_source_has_no_blocking_gpu_wait() {
    let source = include_str!("../vulkan.rs");
    let acquired = source
        .split("fn acquired(&mut self, slot: usize)")
        .nth(1)
        .unwrap()
        .split("fn clear_after_idle")
        .next()
        .unwrap();
    assert!(acquired.contains("self.bins[slot].clear()"));
    assert!(acquired.contains("self.bins[slot].append(&mut self.pending)"));
    for wait in [
        "wait_for_fences(",
        "device_wait_idle(",
        "queue_wait_idle(",
        "while ",
        "loop {",
    ] {
        assert!(
            !acquired.contains(wait),
            "retirement must not block: {wait}"
        );
    }
}

#[test]
fn reset_and_font_reload_source_route_through_whole_texture_retirement() {
    let source = include_str!("../mod.rs");
    for name in ["pub fn reset(&mut self)", "pub fn clear_atlas(&mut self)"] {
        let body = source
            .split(name)
            .nth(1)
            .unwrap()
            .split("\n    }")
            .next()
            .unwrap();
        assert!(body.contains("self.clear_image_textures()"));
        assert!(!body.contains("self.image_textures.clear()"));
    }
    let drain = source
        .split("fn clear_image_textures(&mut self)")
        .nth(1)
        .unwrap()
        .split("\n    }")
        .next()
        .unwrap();
    assert!(drain.contains("std::mem::take(&mut self.image_textures)"));
    assert!(drain.contains("self.retire_image_texture(texture)"));
}

#[test]
fn shutdown_after_idle_releases_pending_and_unsubmitted_frame_bins() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut retirement = SlotRetirement::default();
    retirement.retire(texture(&log));
    retirement.acquired(1);
    // Submission failure panics in present_frame; there is no completion
    // proof for this newly attached bin. Teardown must idle, not advance it.
    retirement.retire(texture(&log));
    assert!(log.borrow().is_empty());
    retirement.clear_after_idle();
    assert_eq!(log.borrow().len(), 4);
    drop(retirement);
    assert_eq!(log.borrow().len(), 4);
}
