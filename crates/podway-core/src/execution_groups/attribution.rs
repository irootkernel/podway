//! Retained actor claims belong to the transition that admitted them.

use super::*;
use crate::ActorAttributionV2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuccessorAttributionTargetV1 {
    InitialGoal,
    RevisedGoal,
    ClosedAttempt,
}

impl ExecutionSessionV3 {
    /// Add caller attribution to the transition just applied by the Engine. The enclosing
    /// mutation commits the attribution with the transition and its original-key outcome.
    pub fn attribute_transition(
        &mut self,
        before: &ExecutionCursorFenceV1,
        target: SuccessorAttributionTargetV1,
        actor: ActorAttributionV2,
    ) -> Result<(), DomainError> {
        if before.session != self.session || before.revision.checked_next()? != self.revision {
            return Err(invalid(
                "attribution does not bind the admitted successor transition",
            ));
        }
        let field = match target {
            SuccessorAttributionTargetV1::InitialGoal
                if before.goal_revision.is_none()
                    && self.goal_revision() == Some(GoalRevisionNumberV2::FIRST) =>
            {
                &mut self.initial_goal_actor
            }
            SuccessorAttributionTargetV1::RevisedGoal => {
                let prior = before.goal_revision.ok_or_else(|| {
                    invalid("revised-goal attribution requires the prior goal revision")
                })?;
                &mut self
                    .goal_revisions
                    .last_mut()
                    .filter(|change| {
                        prior.get().checked_add(1) == Some(change.revision.get())
                            && change.closure.at == self.updated_at
                            && before
                                .attempt
                                .as_ref()
                                .is_none_or(|attempt| attempt == &change.source)
                    })
                    .ok_or_else(|| invalid("attribution does not identify the new goal revision"))?
                    .actor
            }
            SuccessorAttributionTargetV1::ClosedAttempt => {
                let source = before.attempt.as_ref().ok_or_else(|| {
                    invalid("attempt attribution requires the prior scoped attempt")
                })?;
                let index = *self
                    .attempt_ids
                    .get(source.attempt())
                    .ok_or_else(|| invalid("attributed scoped attempt is absent"))?;
                let attempt = &mut self.attempts[index];
                if attempt.target != *source
                    || attempt.closed_at != Some(self.updated_at)
                    || !(attempt.lifecycle == AttemptLifecycle::Completed
                        && attempt.decision.is_some()
                        || attempt.lifecycle == AttemptLifecycle::Abandoned
                            && attempt.reason.is_some())
                {
                    return Err(invalid(
                        "attribution does not bind a decision or rework source",
                    ));
                }
                &mut attempt.closed_actor
            }
            _ => return Err(invalid("attribution does not bind an initial goal")),
        };
        if field.is_some() {
            return Err(invalid("retained actor attribution cannot be overwritten"));
        }
        *field = Some(actor);
        Ok(())
    }
}
