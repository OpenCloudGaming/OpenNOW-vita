//! Fluent-based UI translations, one `.ftl` file per [`Locale`] under `src/i18n/`.

use crate::locale::Locale;
use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use unic_langid::LanguageIdentifier;

type Bundle = FluentBundle<FluentResource>;

pub struct I18n {
    locale: Locale,
}

impl I18n {
    pub fn new(locale: Locale) -> Self {
        Self { locale }
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    /// Resolves `id` in the current locale, falling back to `en-US`, then to `id` itself.
    pub fn text(&self, id: &'static str) -> Rc<str> {
        thread_local! {
            static CACHE: RefCell<HashMap<(Locale, &'static str), Rc<str>>> =
                RefCell::new(HashMap::new());
        }
        CACHE.with(|cell| {
            if let Some(cached) = cell.borrow().get(&(self.locale, id)) {
                return Rc::clone(cached);
            }
            let resolved: Rc<str> = self.text_with_args(id, None).into();
            cell.borrow_mut()
                .insert((self.locale, id), Rc::clone(&resolved));
            resolved
        })
    }

    /// Like [`I18n::text`], with Fluent arguments interpolated into the message.
    pub fn text_with<'a>(&self, id: &'static str, args: FluentArgs<'a>) -> Rc<str> {
        let fingerprint = args_fingerprint(&args);
        thread_local! {
            static CACHE: RefCell<HashMap<(Locale, &'static str, String), Rc<str>>> =
                RefCell::new(HashMap::new());
        }
        CACHE.with(|cell| {
            let key = (self.locale, id, fingerprint);
            if let Some(cached) = cell.borrow().get(&key) {
                return Rc::clone(cached);
            }
            let resolved: Rc<str> = self.text_with_args(id, Some(&args)).into();
            cell.borrow_mut().insert(key, Rc::clone(&resolved));
            resolved
        })
    }

    fn text_with_args(&self, id: &'static str, args: Option<&FluentArgs<'_>>) -> String {
        with_bundle(self.locale, |bundle| format_message(bundle, id, args))
            .or_else(|| with_bundle(Locale::EnUs, |bundle| format_message(bundle, id, args)))
            .unwrap_or_else(|| id.to_owned())
    }
}

/// Wraps an owned string as a [`FluentValue`] argument.
pub fn arg_string(value: impl Into<String>) -> FluentValue<'static> {
    FluentValue::String(value.into().into())
}

fn args_fingerprint(args: &FluentArgs<'_>) -> String {
    let mut out = String::new();
    for (key, value) in args.iter() {
        out.push_str(key);
        out.push('=');
        match value {
            FluentValue::String(s) => out.push_str(s),
            FluentValue::Number(n) => out.push_str(&n.as_string()),
            other => out.push_str(&format!("{other:?}")),
        }
        out.push('\n');
    }
    out
}

fn format_message(
    bundle: &Bundle,
    id: &'static str,
    args: Option<&FluentArgs<'_>>,
) -> Option<String> {
    let message = bundle.get_message(id)?;
    let pattern = message.value()?;
    let mut errors = Vec::new();
    let value = bundle
        .format_pattern(pattern, args, &mut errors)
        .to_string();
    if errors.is_empty() {
        Some(value)
    } else {
        eprintln!("i18n: failed to format {id}: {errors:?}");
        None
    }
}

fn with_bundle<R>(locale: Locale, f: impl FnOnce(&Bundle) -> R) -> R {
    thread_local! {
        static BUNDLES: RefCell<HashMap<Locale, Bundle>> = RefCell::new(HashMap::new());
    }
    BUNDLES.with(|cell| {
        let mut bundles = cell.borrow_mut();
        let bundle = bundles
            .entry(locale)
            .or_insert_with(|| make_bundle(locale, ftl_source(locale)));
        f(bundle)
    })
}

fn ftl_source(locale: Locale) -> &'static str {
    match locale {
        Locale::EnUs => include_str!("i18n/en-US.ftl"),
        Locale::EsEs => include_str!("i18n/es-ES.ftl"),
    }
}

fn make_bundle(locale: Locale, source: &'static str) -> Bundle {
    let langid: LanguageIdentifier = locale
        .as_str()
        .parse()
        .expect("configured locale codes must be valid BCP-47 language identifiers");
    let resource =
        FluentResource::try_new(source.to_owned()).unwrap_or_else(|(resource, errors)| {
            eprintln!("i18n: failed to parse {}: {errors:?}", locale.as_str());
            resource
        });
    let mut bundle = FluentBundle::new(vec![langid]);
    if let Err(errors) = bundle.add_resource(resource) {
        eprintln!(
            "i18n: failed to add {} resource: {errors:?}",
            locale.as_str()
        );
    }
    bundle
}
