use std::{error::Error, fmt, sync::Arc};

use crate::ConstraintKind;

/// Structured diagnostics. Attempted values are deliberately absent from Display.
#[derive(Debug, Clone)]
pub struct ConstraintDiagnostics {
    pub table: &'static str,
    pub constraint: &'static str,
    pub kind: ConstraintKind,
    source: Arc<sqlx::Error>,
}
impl fmt::Display for ConstraintDiagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "constraint violation on {} ({})",
            self.table, self.constraint
        )
    }
}
impl Error for ConstraintDiagnostics {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A known constraint and the typed write input, when it can be attributed.
/// Values may contain PII. Inspect deliberately; default Display never prints them.
#[derive(Debug, Clone)]
pub struct ConstraintConflict<V> {
    pub attempted: Option<V>,
    pub diagnostics: ConstraintDiagnostics,
}
impl<V> ConstraintConflict<V> {
    #[doc(hidden)]
    pub fn new(
        attempted: Option<V>,
        table: &'static str,
        constraint: &'static str,
        kind: ConstraintKind,
        source: sqlx::Error,
    ) -> Self {
        Self {
            attempted,
            diagnostics: ConstraintDiagnostics {
                table,
                constraint,
                kind,
                source: Arc::new(source),
            },
        }
    }
}
impl<V> fmt::Display for ConstraintConflict<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostics.fmt(f)
    }
}
impl<V: fmt::Debug> Error for ConstraintConflict<V> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.diagnostics)
    }
}

/// The primary-key conflict of a create: the id is always known, from the
/// write's own input (a single create) or by matching the database's
/// reported key against the batch's own ids (`create_all`). Unlike
/// [`ConstraintConflict`], `attempted` is never optional here — a pkey
/// violation with no attributable id is `Fatal(Invariant)` instead, never
/// this variant with a guessed or missing id.
#[derive(Debug, Clone)]
pub struct IdConflict<Id> {
    pub attempted: Id,
    pub diagnostics: ConstraintDiagnostics,
}
impl<Id> IdConflict<Id> {
    #[doc(hidden)]
    pub fn new(
        attempted: Id,
        table: &'static str,
        constraint: &'static str,
        kind: ConstraintKind,
        source: sqlx::Error,
    ) -> Self {
        Self {
            attempted,
            diagnostics: ConstraintDiagnostics {
                table,
                constraint,
                kind,
                source: Arc::new(source),
            },
        }
    }
}
impl<Id> fmt::Display for IdConflict<Id> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diagnostics.fmt(f)
    }
}
impl<Id: fmt::Debug> Error for IdConflict<Id> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.diagnostics)
    }
}
