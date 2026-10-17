/// A structural inconsistency in pure Procedure v2 state or declarations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcedureStateErrorV2 {
    reason: &'static str,
}
impl ProcedureStateErrorV2 {
    pub const fn new(reason: &'static str) -> Self {
        Self { reason }
    }
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}
impl std::fmt::Display for ProcedureStateErrorV2 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.reason)
    }
}
impl std::error::Error for ProcedureStateErrorV2 {}
