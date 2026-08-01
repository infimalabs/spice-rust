use crate::admission::{AnswerContent, FoldRegistration, RingRegistration, Source};

macro_rules! registry_source {
    (Fact($variant:ident)) => {
        Source::Fact(wire::FactKind::$variant)
    };
    (Facts($variant:ident => $name:literal)) => {
        Source::Facts {
            plane: stringify!($variant),
            pattern: $name,
        }
    };
    (Fold($variant:ident => $name:literal)) => {
        Source::Fold {
            variant: stringify!($variant),
            name: $name,
        }
    };
    (Head($variant:ident => $name:literal)) => {
        Source::Head {
            variant: stringify!($variant),
            name: $name,
        }
    };
    (Ring($variant:ident => $name:literal)) => {
        Source::Ring {
            variant: stringify!($variant),
            name: $name,
        }
    };
    (Config($variant:ident => $name:literal)) => {
        Source::Config {
            variant: stringify!($variant),
            name: $name,
        }
    };
    (Tape($variant:ident => $name:literal)) => {
        Source::Tape {
            variant: stringify!($variant),
            name: $name,
        }
    };
    (CommandResults($variant:ident => $name:literal)) => {
        Source::CommandResults($name)
    };
}

macro_rules! registry_content {
    (Fact($variant:ident)) => {
        AnswerContent::Fact(wire::FactKind::$variant)
    };
    (Facts($variant:ident => $name:literal)) => {
        AnswerContent::Facts {
            plane: stringify!($variant),
            pattern: $name,
        }
    };
    (Head($variant:ident => $name:literal)) => {
        AnswerContent::Head {
            variant: stringify!($variant),
            name: $name,
        }
    };
}

macro_rules! fold_registry {
    ($(
        $variant:ident => $name:literal {
            answer: $answer:literal;
            content: [$($content_kind:ident $content_args:tt),* $(,)?];
            sources: [$($source_kind:ident $source_args:tt),* $(,)?];
            beyond: $beyond:ident;
        }
    )+) => {
        pub const FOLDS: &[FoldRegistration] = &[
            $(FoldRegistration {
                variant: stringify!($variant),
                name: $name,
                answer: $answer,
                content: &[
                    $(registry_content!($content_kind $content_args)),*
                ],
                sources: &[
                    $(registry_source!($source_kind $source_args)),*
                ],
                beyond: stringify!($beyond),
            }),+
        ];
    };
}

macro_rules! ring_registry {
    ($(
        $variant:ident => $name:literal {
            recovery: $recovery:ident;
        }
    )+) => {
        pub const RINGS: &[RingRegistration] = &[
            $(RingRegistration {
                variant: stringify!($variant),
                name: $name,
                recovery: stringify!($recovery),
            }),+
        ];
    };
}

include!("../src/registry.rs");
