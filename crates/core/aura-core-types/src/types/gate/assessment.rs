//! What Gate's classifier finds about a prompt's work: how much code it
//! changes, how precisely it is asked for, how much must be read first,
//! whether the request says how to check it, and the traps in it. Gate
//! works the work's difficulty out from these.

use serde::{Deserialize, Serialize};

/// What the classifier found about a prompt's work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assessment {
    /// What the work is and what decides whether a model gets it right,
    /// in the classifier's words.
    pub analysis: String,
    pub scope: Scope,
    pub spec: Spec,
    pub exploration: Exploration,
    /// Whether the request gives a way to check the result: a failing
    /// test, an error to reproduce or exact expected outputs.
    pub checkable: bool,
    /// Traps a plausible first attempt falls into, each once, in the
    /// order given; empty when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pitfalls: Vec<Pitfall>,
}

impl Assessment {
    /// What drives its difficulty, as a short list for a table or a log
    /// line: `a few files, clear spec, deep exploring, checkable, traps:
    /// concurrency, security`.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = vec![
            self.scope.shown().to_string(),
            self.spec.shown().to_string(),
            self.exploration.shown().to_string(),
        ];
        if self.checkable {
            parts.push("checkable".to_string());
        }
        if !self.pitfalls.is_empty() {
            let traps: Vec<&str> = self.pitfalls.iter().map(|p| p.shown()).collect();
            parts.push(format!("traps: {}", traps.join(", ")));
        }
        parts.join(", ")
    }
}

/// An enum stored by its snake_case name, with every value in order, its
/// name and how it reads in a summary.
macro_rules! named {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $variant:ident => $stored:literal, $shown:literal;)+ }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vdoc])* $variant,)+
        }

        impl $name {
            /// Every value, in order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Its name as stored.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $stored,)+
                }
            }

            /// How it reads in [`Assessment::summary`].
            #[must_use]
            pub const fn shown(self) -> &'static str {
                match self {
                    $(Self::$variant => $shown,)+
                }
            }

            /// The value named `name` (as [`Self::as_str`] spells it).
            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|v| v.as_str() == name)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

named! {
    /// How much code the work changes, tests left out; smallest first.
    Scope {
        /// A few lines in one place, or no code at all.
        OneSpot => "one_spot", "one spot";
        /// Several places in one file.
        OneFile => "one_file", "one file";
        /// Two to four files.
        FewFiles => "few_files", "a few files";
        /// Five or more files, or a whole module.
        ManyFiles => "many_files", "many files";
    }
}

named! {
    /// How precisely the request says what done looks like; most precise
    /// first.
    Spec {
        /// Inputs and outputs, error messages, names to use.
        Exact => "exact", "exact spec";
        /// What to do, with details left to choose.
        Clear => "clear", "clear spec";
        /// What to build, where or why is left open.
        Loose => "loose", "loose spec";
    }
}

named! {
    /// How much of the codebase must be read before changing it; least
    /// first.
    Exploration {
        /// Everything needed is in the request, or in one file it names.
        None => "none", "no exploring";
        /// A few nearby files.
        Some => "some", "some exploring";
        /// Unfamiliar code across the codebase.
        Deep => "deep", "deep exploring";
    }
}

named! {
    /// A kind of trap a plausible first attempt falls into.
    Pitfall {
        /// Time zones, daylight saving, calendars.
        DatesTimes => "dates_times", "dates and times";
        /// Rounding, precision, currency.
        MoneyNumbers => "money_numbers", "money and numbers";
        /// Unicode, display width, byte lengths.
        TextEncoding => "text_encoding", "text encoding";
        /// Races, ordering, locking, async.
        Concurrency => "concurrency", "concurrency";
        /// Escaping, injection, permissions, secrets.
        Security => "security", "security";
        /// Grammars, formats, malformed input.
        Parsing => "parsing", "parsing";
        /// Public APIs, versions, platforms, migrations.
        Compatibility => "compatibility", "compatibility";
        /// Complexity, memory, hot paths.
        Performance => "performance", "performance";
    }
}
