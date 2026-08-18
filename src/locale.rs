#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Locale {
    #[default]
    EnUs,
    EsEs,
    FrFr,
    RuRu,
}

impl Locale {
    pub const ALL: [Locale; 4] = [Self::EnUs, Self::EsEs, Self::FrFr, Self::RuRu];

    /// `(locale code, store market, native-language label)`.
    fn info(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::EnUs => ("en-US", "US", "English (US)"),
            Self::EsEs => ("es-ES", "ES", "Español (España)"),
            Self::FrFr => ("fr-FR", "FR", "Français (France)"),
            Self::RuRu => ("ru-RU", "RU", "Русский"),
        }
    }

    /// The locale code sent to xCloud, e.g.
    pub fn as_str(self) -> &'static str {
        self.info().0
    }

    /// Native-language label shown in the language picker, e.g.
    pub fn label(self) -> &'static str {
        self.info().2
    }
}
