//! Enums a choice or score question can answer with.

/// One option of a [`Choices`] enum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChoiceOption<T> {
    /// The enum value this option stands for.
    pub value: T,
    /// The label Jev sees and answers with. Unique within the enum.
    pub label: &'static str,
    /// What the option means, when the label alone is not enough.
    pub description: Option<&'static str>,
}

/// One level of a [`Levels`] enum.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level<T> {
    /// The enum value this level stands for.
    pub value: T,
    /// The text Jev sees.
    pub text: &'static str,
}

/// An enum Jev can pick with a choice question. Jev slightly favors options listed first.
pub trait Choices: Copy + PartialEq + 'static {
    /// Every option in the order Jev sees them.
    const OPTIONS: &'static [ChoiceOption<Self>];
}

/// An enum for a score question's ordered levels.
pub trait Levels: Copy + PartialEq + 'static {
    /// Every level, lowest first.
    const LEVELS: &'static [Level<Self>];
}
