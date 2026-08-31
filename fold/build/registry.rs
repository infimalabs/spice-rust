use crate::admission::{FoldRegistration, Source};

macro_rules! registry_source {
    (Facts($variant:ident => $name:literal)) => {
        Source::Facts {
            plane: stringify!($variant),
            pattern: $name,
        }
    };
}

macro_rules! fold_registry {
    ($(
        $variant:ident => $name:literal {
            answer: $answer:literal;
            sources: [$($source_kind:ident $source_args:tt),* $(,)?];
            beyond: $beyond:ident;
        }
    )+) => {
        pub const FOLDS: &[FoldRegistration] = &[
            $(FoldRegistration {
                variant: stringify!($variant),
                name: $name,
                answer: $answer,
                sources: &[
                    $(registry_source!($source_kind $source_args)),*
                ],
                beyond: stringify!($beyond),
            }),+
        ];
    };
}

// Textual inclusion is the contract here: the same declaration-only registry
// feeds build-time admission and the runtime inventory without a second copy.
include!("../src/registry.rs");
