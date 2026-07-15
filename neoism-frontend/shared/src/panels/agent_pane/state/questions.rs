use super::*;

use crate::panels::agent_pane::question_policy::{
    NeoismAgentPendingQuestion, QuestionCommit,
};

impl NeoismAgentPane {
    pub fn pending_question(&self) -> Option<&NeoismAgentPendingQuestion> {
        self.pending_question.as_ref()
    }

    pub fn register_question_option_rect(&mut self, index: usize, rect: [f32; 4]) {
        self.question_option_hit_rects.push((index, rect));
    }

    pub fn clear_question_option_rects(&mut self) {
        self.question_option_hit_rects.clear();
    }

    pub fn set_prompt_picker_rect(&mut self, rect: Option<[f32; 4]>) {
        self.prompt_picker_rect = rect;
    }

    pub fn prompt_picker_rect(&self) -> Option<[f32; 4]> {
        self.prompt_picker_rect
    }

    pub fn enqueue_pending_question(&mut self, question: NeoismAgentPendingQuestion) {
        permission_policy::enqueue_permission(
            &mut self.pending_question,
            &mut self.pending_question_queue,
            question,
            |question| question.id.as_str(),
        );
    }

    pub fn remove_pending_question(&mut self, request_id: &str) -> bool {
        permission_policy::remove_permission(
            &mut self.pending_question,
            &mut self.pending_question_queue,
            request_id,
            |question| question.id.as_str(),
        )
    }

    pub fn move_question_selection(&mut self, delta: isize) -> bool {
        let Some(question) = self.pending_question.as_mut() else {
            return false;
        };
        if question.responding {
            return true;
        }
        question.move_selection(delta);
        true
    }

    pub fn question_type_str(&mut self, text: &str) -> bool {
        let Some(question) = self.pending_question.as_mut() else {
            return false;
        };
        if !question.responding {
            question.type_str(text);
        }
        true
    }

    pub fn question_backspace(&mut self) -> bool {
        let Some(question) = self.pending_question.as_mut() else {
            return false;
        };
        if !question.responding {
            question.backspace();
        }
        true
    }

    /// Click on a prompt-picker row — select and commit it.
    pub fn respond_question_at(&mut self, x: f32, y: f32) -> bool {
        let Some(index) = permission_policy::choice_at(
            self.question_option_hit_rects.iter().copied(),
            x,
            y,
        ) else {
            return false;
        };
        if let Some(question) = self.pending_question.as_mut() {
            if !question.responding {
                question.selected = index;
            }
        }
        self.submit_pending_question()
    }

    /// Commit the selected row for the current question; when the last
    /// question of the request is answered, send the reply.
    pub fn submit_pending_question(&mut self) -> bool {
        let Some(question) = self.pending_question.as_mut() else {
            return false;
        };
        if question.responding {
            return true;
        }
        match question.commit_selected() {
            QuestionCommit::Nothing | QuestionCommit::Advanced => true,
            QuestionCommit::Finished(answers) => {
                question.responding = true;
                let id = question.id.clone();
                if id.is_empty() {
                    self.system_message("Question", "pending question has no id");
                    self.clear_pending_question_current();
                } else {
                    self.push_outbound(OutboundAgentCommand::ReplyQuestion {
                        id,
                        answers,
                    });
                }
                true
            }
        }
    }

    /// Esc — reject the pending question so the model's run resumes with
    /// a "user declined to answer" error instead of parking forever.
    pub fn reject_pending_question(&mut self) -> bool {
        let Some(question) = self.pending_question.as_mut() else {
            return false;
        };
        if question.responding {
            return true;
        }
        question.responding = true;
        let id = question.id.clone();
        if id.is_empty() {
            self.clear_pending_question_current();
        } else {
            self.push_outbound(OutboundAgentCommand::RejectQuestion { id });
        }
        true
    }

    pub fn question_reply_succeeded(&mut self, id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        if self
            .pending_question
            .as_ref()
            .is_some_and(|question| question.id == id)
        {
            self.clear_pending_question_current();
            return true;
        }
        self.remove_pending_question(id)
    }

    pub fn question_reply_failed(&mut self, id: &str, error: impl Into<String>) -> bool {
        let changed = permission_policy::fail_reply(
            &mut self.pending_question,
            id,
            |question| question.id.as_str(),
            |question, responding| question.responding = responding,
        );
        if changed {
            self.system_message("Question", error.into());
        }
        changed
    }

    pub(in crate::panels::agent_pane::state) fn clear_pending_question_current(
        &mut self,
    ) {
        permission_policy::clear_current_permission(
            &mut self.pending_question,
            &mut self.pending_question_queue,
        );
    }
}
