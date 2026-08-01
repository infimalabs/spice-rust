#[path = "build/admission.rs"]
mod admission;

use admission::{AnswerContent, FactRegistration, FoldRegistration, RingRegistration, Source};
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Ident, Item, LitStr, Token};

macro_rules! registry_source {
    (Fact($variant:ident => $name:literal)) => {
        Source::Fact {
            variant: stringify!($variant),
            kind: $name,
        }
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
    (Fact($variant:ident => $name:literal)) => {
        AnswerContent::Fact {
            variant: stringify!($variant),
            kind: $name,
        }
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
            content: [$($content_kind:ident($content_variant:ident => $content_name:literal)),* $(,)?];
            sources: [$($source_kind:ident($source_variant:ident => $source_name:literal)),* $(,)?];
            beyond: $beyond:ident;
        }
    )+) => {
        const FOLDS: &[FoldRegistration] = &[
            $(FoldRegistration {
                variant: stringify!($variant),
                name: $name,
                answer: $answer,
                content: &[
                    $(registry_content!($content_kind($content_variant => $content_name))),*
                ],
                sources: &[
                    $(registry_source!($source_kind($source_variant => $source_name))),*
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
        const RINGS: &[RingRegistration] = &[
            $(RingRegistration {
                variant: stringify!($variant),
                name: $name,
                recovery: stringify!($recovery),
            }),+
        ];
    };
}

include!("src/registry.rs");

struct Vocabulary(Vec<FactRegistration>);

impl Parse for Vocabulary {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut facts = Vec::new();
        while !input.is_empty() {
            let _attributes = input.call(Attribute::parse_outer)?;
            let variant = input.parse::<Ident>()?;
            input.parse::<Token![=]>()?;
            let kind = input.parse::<LitStr>()?;
            let producer = if input.peek(Token![;]) {
                None
            } else {
                let by = input.parse::<Ident>()?;
                if by != "by" {
                    return Err(syn::Error::new(by.span(), "expected `by Producer`"));
                }
                Some(input.parse::<Ident>()?.to_string())
            };
            input.parse::<Token![;]>()?;
            facts.push(FactRegistration {
                variant: variant.to_string(),
                kind: kind.value(),
                producer,
            });
        }
        Ok(Self(facts))
    }
}

fn fact_vocabulary() -> Vec<FactRegistration> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("wire")
        .join("src")
        .join("fact.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let file = syn::parse_file(&source)
        .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
    let vocabulary = file.items.into_iter().find_map(|item| match item {
        Item::Macro(item) if item.mac.path.is_ident("vocabulary") => Some(item.mac.tokens),
        _ => None,
    });
    let tokens = vocabulary.unwrap_or_else(|| {
        panic!("{} contains no vocabulary! declaration", path.display());
    });
    syn::parse2::<Vocabulary>(tokens)
        .unwrap_or_else(|error| panic!("cannot parse wire fact vocabulary: {error}"))
        .0
}

fn main() {
    println!("cargo::rerun-if-changed=build/admission.rs");
    println!("cargo::rerun-if-changed=src/registry.rs");
    println!("cargo::rerun-if-changed=../wire/src/fact.rs");

    let facts = fact_vocabulary();
    admission::validate(&facts, FOLDS, RINGS).unwrap_or_else(|refusal| {
        panic!("generated fold registry refused: {refusal}");
    });
}
