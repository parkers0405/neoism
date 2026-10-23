use std::rc::Rc;

/// A submitted user turn and its position in the displayed conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineQuestionTarget {
    pub source_index: usize,
    pub message_id: String,
    pub top: f32,
    pub assistant_source_index: Option<usize>,
}

pub type TimelineQuestionTargets = Rc<Vec<TimelineQuestionTarget>>;

/// Questions are evenly spaced on the rail, not proportional to answer length.
pub fn question_index_at(
    track: [f32; 4],
    count: usize,
    pointer: Option<(f32, f32)>,
) -> Option<usize> {
    let (x, y) = pointer?;
    if count == 0 || !super::interaction_policy::rect_contains(track, x, y) {
        return None;
    }
    if count == 1 || track[3] <= 0.0 {
        return Some(0);
    }
    Some(
        (((y - track[1]) / track[3]).clamp(0.0, 1.0) * (count - 1) as f32).round()
            as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::question_index_at;

    #[test]
    fn question_hits_follow_turn_count_not_content_height_or_an_eleven_tick_cap() {
        let track = [400.0, 100.0, 20.0, 160.0];
        for count in [2, 3, 11, 21, 80] {
            for index in 0..count {
                let y = track[1] + track[3] * index as f32 / (count - 1) as f32;
                assert_eq!(
                    question_index_at(track, count, Some((410.0, y))),
                    Some(index)
                );
            }
        }
    }

    #[test]
    fn question_hits_clear_outside_the_gutter_and_handle_empty_or_single_turns() {
        let track = [400.0, 100.0, 20.0, 160.0];
        assert_eq!(question_index_at(track, 0, Some((410.0, 180.0))), None);
        assert_eq!(question_index_at(track, 1, Some((410.0, 180.0))), Some(0));
        assert_eq!(question_index_at(track, 20, None), None);
        assert_eq!(question_index_at(track, 20, Some((399.0, 180.0))), None);
        assert_eq!(question_index_at(track, 20, Some((421.0, 180.0))), None);
        assert_eq!(question_index_at(track, 20, Some((410.0, 99.0))), None);
        assert_eq!(question_index_at(track, 20, Some((410.0, 261.0))), None);
        assert_eq!(question_index_at(track, 20, Some((f32::NAN, 180.0))), None);
    }
}
