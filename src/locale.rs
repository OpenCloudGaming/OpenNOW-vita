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

    pub fn from_str(code: &str) -> Self {
        let code = code.trim();
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.as_str() == code)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn every_locale_round_trips_through_as_str() {
        for locale in Locale::ALL {
            assert_eq!(Locale::from_str(locale.as_str()), locale);
        }
    }

    #[test]
    fn unknown_or_empty_codes_fall_back_to_english() {
        assert_eq!(Locale::from_str(""), Locale::EnUs);
        assert_eq!(Locale::from_str("  "), Locale::EnUs);
        assert_eq!(Locale::from_str("zz-ZZ"), Locale::EnUs);
        assert_eq!(Locale::from_str(" ru-RU "), Locale::RuRu);
        assert_eq!(Locale::from_str(" es-ES "), Locale::EsEs);
    }
}
