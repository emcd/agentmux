use std::{
    error::Error,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
};

/// Separator between layers in the environment form of the layer list.
///
/// Matches every other Unix search variable, which is also why a path
/// containing this character cannot be expressed through the environment. The
/// repeatable flag is the escape hatch.
pub const LAYER_SEPARATOR: char = ':';

/// An ordered, non-empty list of configuration roots, searched front to back.
///
/// These are search-path semantics, not cascade semantics: a lookup finds a
/// file in the first layer holding it and stops, so the first layer is the
/// highest-precedence one and the last is the base. Nothing is merged.
///
/// A supplied list is closed — no root outside it is consulted for any file.
/// That is about which roots are searched, not about what absence means: an
/// artifact absent from every layer keeps whatever absence semantics it already
/// had, so optional files stay optional.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigurationRoots {
    layers: Vec<PathBuf>,
}

impl ConfigurationRoots {
    /// Builds a single-layer list.
    ///
    /// The XDG/home default resolves through this, so the defaulted tier and an
    /// operator-declared list are the same shape and one lookup path serves
    /// both.
    #[must_use]
    pub fn single(root: impl Into<PathBuf>) -> Self {
        Self {
            layers: vec![root.into()],
        }
    }

    /// Builds a list from operator-supplied elements, in the order given.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationRootsError`] when the list is empty or any
    /// element is.
    pub fn from_elements(
        elements: impl IntoIterator<Item = PathBuf>,
    ) -> Result<Self, ConfigurationRootsError> {
        let layers = elements.into_iter().collect::<Vec<_>>();
        if layers.is_empty() {
            return Err(ConfigurationRootsError::EmptyList);
        }
        for (position, layer) in layers.iter().enumerate() {
            if layer.as_os_str().is_empty() {
                return Err(ConfigurationRootsError::EmptyElement { position });
            }
        }
        Ok(Self { layers })
    }

    /// Parses the separator-delimited environment form.
    ///
    /// A value that is empty or entirely whitespace yields `None`, leaving the
    /// variable to behave as unset so resolution falls to the tier below.
    /// Anything else is split without trimming the individual elements, since a
    /// trailing space is a legal part of a path on this platform.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigurationRootsError`] when a leading, trailing, or doubled
    /// separator contributes an empty element.
    pub fn from_environment_value(value: &str) -> Result<Option<Self>, ConfigurationRootsError> {
        if value.trim().is_empty() {
            return Ok(None);
        }
        Self::from_elements(value.split(LAYER_SEPARATOR).map(PathBuf::from)).map(Some)
    }

    /// The layers, highest precedence first.
    #[must_use]
    pub fn layers(&self) -> &[PathBuf] {
        &self.layers
    }

    /// The lowest-precedence layer, which every lookup falls through to.
    ///
    /// This is where a file absent from every layer is reported against. The
    /// base is the shared layer an override layer overrides, so it is the
    /// creation site a reader would infer; naming the override layer instead
    /// would suggest a file must be created in the layer that exists to shadow
    /// one.
    #[must_use]
    pub fn base_layer(&self) -> &Path {
        // The constructors reject an empty list, so this cannot fail.
        self.layers.last().expect("layer list is never empty")
    }
}

/// Rejection of a malformed configuration layer list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigurationRootsError {
    EmptyList,
    /// An empty element, from a flag with an empty value or from a leading,
    /// trailing, or doubled separator in the environment form.
    EmptyElement {
        position: usize,
    },
}

impl Display for ConfigurationRootsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyList => write!(formatter, "configuration layer list is empty"),
            // The search-path convention this design otherwise follows reads an
            // empty element as the working directory. Configuration selects
            // policies and identities, so reading a layer from wherever a
            // process was started is a privilege question rather than a
            // convenience.
            Self::EmptyElement { position } => write!(
                formatter,
                "configuration layer {} is empty; an empty layer is rejected rather than \
                 read as the working directory",
                position + 1
            ),
        }
    }
}

impl Error for ConfigurationRootsError {}
