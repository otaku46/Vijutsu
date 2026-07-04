#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewMode {
    FollowCurrent,
    Historical(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Current,
    FilesystemAhead,
    HeadChanged,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Inspect,
    MutateCurrent,
    ImportFromHistorical,
    SurgeryOperationGraph,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationContext {
    pub view_mode: ViewMode,
    pub observed_operation: String,
    pub current_operation: String,
    pub freshness: Freshness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandBlocked {
    pub reason: &'static str,
}

pub fn guard_command(
    context: &ObservationContext,
    command: CommandKind,
) -> Result<(), CommandBlocked> {
    if command == CommandKind::Inspect {
        return Ok(());
    }
    if context.freshness != Freshness::Current {
        return Err(CommandBlocked {
            reason: "view is not fresh; refresh before mutating",
        });
    }
    if command == CommandKind::ImportFromHistorical {
        return match context.view_mode {
            ViewMode::Historical(_) => Ok(()),
            ViewMode::FollowCurrent => Err(CommandBlocked {
                reason: "import requires a historical source view",
            }),
        };
    }
    if context.observed_operation != context.current_operation {
        return Err(CommandBlocked {
            reason: "observed operation no longer matches current operation",
        });
    }
    if matches!(context.view_mode, ViewMode::Historical(_)) {
        return Err(CommandBlocked {
            reason: "historical view is read-only",
        });
    }
    Ok(())
}

pub fn guard_execution_head(
    preview_operation: &str,
    current_operation: &str,
) -> Result<(), CommandBlocked> {
    if preview_operation == current_operation {
        Ok(())
    } else {
        Err(CommandBlocked {
            reason: "operation head changed after preview; rebuild preview before executing",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(view_mode: ViewMode, freshness: Freshness) -> ObservationContext {
        ObservationContext {
            view_mode,
            observed_operation: "op-current".to_string(),
            current_operation: "op-current".to_string(),
            freshness,
        }
    }

    fn historical_context(freshness: Freshness) -> ObservationContext {
        ObservationContext {
            view_mode: ViewMode::Historical("op-old".to_string()),
            observed_operation: "op-old".to_string(),
            current_operation: "op-current".to_string(),
            freshness,
        }
    }

    #[test]
    fn historical_mutate_current_is_rejected() {
        let context = context(
            ViewMode::Historical("op-old".to_string()),
            Freshness::Current,
        );

        assert_eq!(
            guard_command(&context, CommandKind::MutateCurrent),
            Err(CommandBlocked {
                reason: "historical view is read-only",
            })
        );
    }

    #[test]
    fn follow_current_mutate_current_is_allowed_when_fresh() {
        let context = context(ViewMode::FollowCurrent, Freshness::Current);

        assert_eq!(guard_command(&context, CommandKind::MutateCurrent), Ok(()));
    }

    #[test]
    fn historical_import_is_allowed_when_fresh() {
        let context = historical_context(Freshness::Current);

        assert_eq!(
            guard_command(&context, CommandKind::ImportFromHistorical),
            Ok(())
        );
    }

    #[test]
    fn current_import_without_historical_source_is_rejected() {
        let context = context(ViewMode::FollowCurrent, Freshness::Current);

        assert_eq!(
            guard_command(&context, CommandKind::ImportFromHistorical),
            Err(CommandBlocked {
                reason: "import requires a historical source view",
            })
        );
    }

    #[test]
    fn non_current_freshness_rejects_mutation() {
        for freshness in [
            Freshness::FilesystemAhead,
            Freshness::HeadChanged,
            Freshness::Stale,
        ] {
            let context = context(ViewMode::FollowCurrent, freshness);
            assert_eq!(
                guard_command(&context, CommandKind::MutateCurrent),
                Err(CommandBlocked {
                    reason: "view is not fresh; refresh before mutating",
                })
            );
        }
    }

    #[test]
    fn stale_preview_head_rejects_execution() {
        assert_eq!(
            guard_execution_head("op-a", "op-b"),
            Err(CommandBlocked {
                reason: "operation head changed after preview; rebuild preview before executing",
            })
        );
    }
}
