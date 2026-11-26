//! The fields of the channel form.

/// A field of the form, in the order Tab walks them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ChannelField {
    /// Short unique name the channel is addressed by
    #[default]
    Slug,
    /// What the channel is called
    Title,
    /// What the channel is for
    Description,
    /// The project the channel's agents work in
    Project,
    /// Where the members the channel starts run
    Placement,
}

impl ChannelField {
    /// Every field, top to bottom.
    pub(crate) const ALL: [Self; 5] = [
        Self::Slug,
        Self::Title,
        Self::Description,
        Self::Project,
        Self::Placement,
    ];

    /// What the field's label reads.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Slug => "Slug",
            Self::Title => "Title",
            Self::Description => "Description",
            Self::Project => "Project",
            Self::Placement => "Agents run",
        }
    }

    /// Where the field's text is kept, for the fields that are text.
    pub(super) const fn text_index(self) -> Option<usize> {
        match self {
            Self::Slug => Some(0),
            Self::Title => Some(1),
            Self::Description => Some(2),
            Self::Project | Self::Placement => None,
        }
    }

    pub(super) const fn next(self) -> Self {
        match self {
            Self::Slug => Self::Title,
            Self::Title => Self::Description,
            Self::Description => Self::Project,
            Self::Project => Self::Placement,
            Self::Placement => Self::Slug,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Slug => Self::Placement,
            Self::Title => Self::Slug,
            Self::Description => Self::Title,
            Self::Project => Self::Description,
            Self::Placement => Self::Project,
        }
    }
}
