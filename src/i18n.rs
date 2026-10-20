//! Localization catalogs and an immutable translation service.
//!
//! Built-in catalogs are embedded with [`include_str!`], so the module works
//! in both GUI and console-only builds. External catalogs may be discovered in
//! a `locales/` directory next to the executable or supplied explicitly with
//! [`LoadOptions::language_file`]. External files are merged over the built-in
//! catalog with the same locale; missing messages fall back to the declared
//! built-in fallback (English by default).
//!
//! Messages use flat quoted semantic keys and named placeholders:
//!
//! ```toml
//! [messages]
//! "gui.progress.files" = "{completed} / {total} files"
//! ```
//!
//! `{{` and `}}` render literal braces. Placeholder contents are never
//! evaluated as expressions.
//!
//! Errors that need translation are represented by [`LocalizedMessage`], which
//! keeps the key and named parameters instead of a formatted Chinese string.
//! Its [`Display`](std::fmt::Display) implementation deliberately renders the
//! Chinese legacy text so it remains usable with `?` and `anyhow` before a
//! selected service is available. Use [`I18n::render_anyhow`] to render a full
//! error chain in the selected language while preserving details from the OS
//! or third-party libraries.

use anyhow::Error as AnyhowError;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// Catalog schema version understood by this module.
pub const SCHEMA_VERSION: u32 = 1;
/// Locale selected by [`I18n::default`].
pub const DEFAULT_LOCALE: &str = "zh-CN";
/// Locale used when no more specific fallback is declared.
pub const FALLBACK_LOCALE: &str = "en-US";
/// Locales embedded in the executable. External catalogs may only declare one
/// of these as their fallback.
pub const BUILT_IN_LOCALES: [&str; 2] = ["en-US", "zh-CN"];

const ZH_CN_SOURCE: &str = include_str!("../resources/locales/zh-CN.toml");
const EN_US_SOURCE: &str = include_str!("../resources/locales/en-US.toml");
const TOP_LEVEL_KEYS: [&str; 5] = ["schema_version", "locale", "name", "fallback", "messages"];

/// Result type used by catalog loading and selection.
pub type I18nResult<T> = std::result::Result<T, LocalizedMessage>;

/// A translated error or diagnostic that keeps its semantic key and named
/// parameters.
///
/// The [`Display`](std::fmt::Display) implementation renders the built-in
/// Chinese catalog so existing error paths keep their legacy default. Prefer
/// [`I18n::translate_message`] or [`I18n::render_anyhow`] whenever a selected
/// service is available.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalizedMessage {
    key: String,
    parameters: BTreeMap<String, String>,
}

impl LocalizedMessage {
    /// Create a message for `key` with no parameters.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            parameters: BTreeMap::new(),
        }
    }

    /// Attach a named placeholder value. Later values with the same name
    /// replace earlier ones.
    pub fn with_param(mut self, name: impl Into<String>, value: impl fmt::Display) -> Self {
        self.parameters.insert(name.into(), value.to_string());
        self
    }

    /// Semantic catalog key.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Named placeholder values.
    pub fn parameters(&self) -> &BTreeMap<String, String> {
        &self.parameters
    }
}

impl fmt::Display for LocalizedMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match legacy_messages().get(&self.key) {
            Some(template) => formatter.write_str(&template.render(&self.parameters)),
            None => formatter.write_str(&self.key),
        }
    }
}

impl std::error::Error for LocalizedMessage {}

/// One available language, in the form exposed by
/// [`I18n::available_languages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Language {
    /// Locale identifier such as `zh-CN`.
    pub id: String,
    /// Display name from the catalog's `name` field.
    pub name: String,
    /// Whether this locale ships with the executable.
    pub built_in: bool,
}

/// Structured kinds for non-fatal loader diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WarningKind {
    /// The locales directory could not be enumerated.
    DirectoryUnreadable,
    /// An individual language file could not be read.
    FileUnreadable,
    /// The TOML was malformed (including duplicate keys).
    MalformedPack,
    /// `schema_version` was missing or unsupported.
    SchemaVersion,
    /// The locale identifier was missing, invalid, or unsafe.
    LocaleInvalid,
    /// The declared locale did not match the file name.
    LocaleMismatch,
    /// The fallback was invalid or not a built-in locale.
    FallbackInvalid,
    /// Another catalog for the same locale was already loaded.
    DuplicateLocale,
    /// A message value was not a string.
    ValueType,
    /// An unknown top-level key was ignored.
    UnknownTopLevelKey,
    /// An unknown message key was ignored.
    UnknownKey,
    /// A message contained an invalid placeholder and was ignored.
    InvalidPlaceholder,
    /// A message's placeholders did not match the canonical catalog and it
    /// was ignored.
    PlaceholderMismatch,
}

/// A non-fatal localization loader diagnostic.
///
/// The message is stored as a [`LocalizedMessage`], so it can be rendered in
/// the selected language with [`I18n::warning_text`]. Its `Display`
/// implementation uses the Chinese legacy default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    kind: WarningKind,
    path: Option<PathBuf>,
    locale: Option<String>,
    message: LocalizedMessage,
}

impl Warning {
    /// Structured diagnostic kind.
    pub fn kind(&self) -> WarningKind {
        self.kind
    }

    /// File or directory involved, when known.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Locale involved, when known.
    pub fn locale(&self) -> Option<&str> {
        self.locale.as_deref()
    }

    /// Typed message carrying the catalog key and parameters.
    pub fn message(&self) -> &LocalizedMessage {
        &self.message
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

/// Options for [`I18n::load`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Discover `*.toml` files. With `locales_dir` unset, the platform's
    /// executable-adjacent `locales/` directory is used.
    pub discover: bool,
    /// Custom directory to scan when `discover` is true.
    pub locales_dir: Option<PathBuf>,
    /// An explicit catalog. Its declared locale must match the selected
    /// locale. Explicit messages override discovered and built-in messages.
    pub language_file: Option<PathBuf>,
}

impl LoadOptions {
    /// Discover catalogs next to the executable.
    pub fn discovered() -> Self {
        Self {
            discover: true,
            ..Self::default()
        }
    }

    /// Discover catalogs in a custom directory.
    pub fn with_locales_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.discover = true;
        self.locales_dir = Some(directory.into());
        self
    }

    /// Use an explicit language file.
    pub fn with_language_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.language_file = Some(path.into());
        self
    }
}

/// Immutable catalogs plus the explicitly selected locale.
///
/// The service owns a snapshot of the built-in catalogs, discovered external
/// catalogs, and loader warnings. Changing the language returns a new service
/// via [`select`](I18n::select); no mutable global state is used.
#[derive(Debug, Clone)]
pub struct I18n {
    selected: String,
    builtins: BTreeMap<String, Catalog>,
    external: BTreeMap<String, Catalog>,
    warnings: Vec<Warning>,
}

impl Default for I18n {
    fn default() -> Self {
        Self::with_locale(DEFAULT_LOCALE).unwrap_or_else(|_| Self {
            selected: DEFAULT_LOCALE.to_string(),
            builtins: BTreeMap::new(),
            external: BTreeMap::new(),
            warnings: Vec::new(),
        })
    }
}

impl I18n {
    /// Load the built-in catalogs for `selected` without file discovery.
    pub fn with_locale(selected: &str) -> I18nResult<Self> {
        Self::load(selected, &LoadOptions::default())
    }

    /// Load the built-in catalogs and discover `locales/*.toml` next to the
    /// executable.
    pub fn discover(selected: &str) -> I18nResult<Self> {
        Self::load(selected, &LoadOptions::discovered())
    }

    /// Load catalogs according to `options`.
    ///
    /// Invalid discovered packs are skipped with warnings. An explicitly
    /// supplied language file is strict: any invalid catalog returns an error.
    pub fn load(selected: &str, options: &LoadOptions) -> I18nResult<Self> {
        validate_locale(selected)?;
        let builtins = load_builtins()?;
        let canonical = canonical_placeholders(&builtins);
        let mut warnings = Vec::new();
        let mut external = BTreeMap::new();
        if options.discover {
            let directory = options.locales_dir.clone().or_else(default_locales_dir);
            if let Some(directory) = directory {
                discover_locales(&directory, &canonical, &mut external, &mut warnings);
            }
        }
        if let Some(path) = &options.language_file {
            let catalog = load_explicit(path, selected, &canonical, &mut warnings)?;
            external.insert(catalog.locale.clone(), catalog);
        }
        Ok(Self {
            selected: selected.to_string(),
            builtins,
            external,
            warnings,
        })
    }

    /// Load an explicit language file and select the locale declared inside
    /// it. Discovery is not performed.
    pub fn from_language_file(path: impl Into<PathBuf>) -> I18nResult<Self> {
        let path = path.into();
        let text = fs::read_to_string(&path).map_err(|error| unreadable_file(&path, error))?;
        let locale = declared_locale(&text).map_err(|detail| invalid_file(&path, detail))?;
        Self::load(&locale, &LoadOptions::default().with_language_file(path))
    }

    /// Return a copy of this service with another selected locale.
    ///
    /// Any syntactically valid identifier is accepted; a locale with no
    /// catalog falls back to English. Use [`has_locale`](I18n::has_locale) or
    /// [`available_languages`](I18n::available_languages) to check
    /// availability.
    pub fn select(&self, locale: &str) -> I18nResult<Self> {
        validate_locale(locale)?;
        let mut selected = self.clone();
        selected.selected = locale.to_string();
        Ok(selected)
    }

    /// Apply a saved GUI locale preference.
    ///
    /// Missing, malformed, or unavailable preferences are ignored and the
    /// current selection is kept, so stale settings never prevent startup.
    pub fn select_saved(&self, preference: Option<&str>) -> I18nResult<Self> {
        match preference {
            Some(locale) if is_valid_locale(locale) && self.has_locale(locale) => {
                self.select(locale)
            }
            _ => Ok(self.clone()),
        }
    }

    /// Currently selected locale identifier.
    pub fn selected_locale(&self) -> &str {
        &self.selected
    }

    /// Whether a catalog exists for `locale` (built-in or discovered).
    pub fn has_locale(&self, locale: &str) -> bool {
        self.builtins.contains_key(locale) || self.external.contains_key(locale)
    }

    /// Built-in and valid external languages, sorted by identifier.
    pub fn available_languages(&self) -> Vec<Language> {
        let mut identifiers: BTreeSet<&str> = self.builtins.keys().map(String::as_str).collect();
        identifiers.extend(self.external.keys().map(String::as_str));
        identifiers
            .into_iter()
            .map(|identifier| {
                let name = self
                    .external
                    .get(identifier)
                    .map(|catalog| catalog.name.clone())
                    .or_else(|| {
                        self.builtins
                            .get(identifier)
                            .map(|catalog| catalog.name.clone())
                    })
                    .unwrap_or_else(|| identifier.to_string());
                Language {
                    id: identifier.to_string(),
                    name,
                    built_in: self.builtins.contains_key(identifier),
                }
            })
            .collect()
    }

    /// Non-fatal diagnostics produced while loading external catalogs.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Render a loader warning in the selected language.
    pub fn warning_text(&self, warning: &Warning) -> String {
        self.translate_message(&warning.message)
    }

    /// Render all loader warnings in the selected language.
    pub fn warning_texts(&self) -> Vec<String> {
        self.warnings
            .iter()
            .map(|warning| self.warning_text(warning))
            .collect()
    }

    /// Translate a static message.
    ///
    /// Catalog placeholders are left untouched; use
    /// [`translate_with`](I18n::translate_with) when values are available.
    /// Unknown keys return the key itself instead of panicking.
    pub fn translate(&self, key: &str) -> String {
        match self.template(key) {
            Some(template) => template.render(&BTreeMap::new()),
            None => key.to_string(),
        }
    }

    /// Translate a message and substitute named placeholders.
    ///
    /// Missing parameters keep their `{name}` form; extra parameters are
    /// ignored. No expression syntax is evaluated.
    pub fn translate_with(&self, key: &str, parameters: &[(&str, &str)]) -> String {
        let values: BTreeMap<String, String> = parameters
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        match self.template(key) {
            Some(template) => template.render(&values),
            None => key.to_string(),
        }
    }

    /// Translate a typed [`LocalizedMessage`].
    pub fn translate_message(&self, message: &LocalizedMessage) -> String {
        match self.template(message.key()) {
            Some(template) => template.render(message.parameters()),
            None => message.key().to_string(),
        }
    }

    /// Render an [`anyhow`] error chain in the selected language.
    ///
    /// Every cause that is a [`LocalizedMessage`] is translated; all other
    /// causes are rendered with their own `Display` implementation, preserving
    /// OS and third-party error details verbatim.
    pub fn render_anyhow(&self, error: &AnyhowError) -> String {
        let separator = self.translate("error.detail_separator");
        let mut parts = Vec::new();
        for cause in error.chain() {
            if let Some(message) = cause.downcast_ref::<LocalizedMessage>() {
                parts.push(self.translate_message(message));
            } else {
                parts.push(cause.to_string());
            }
        }
        if parts.is_empty() {
            error.to_string()
        } else {
            parts.join(&separator)
        }
    }

    fn template(&self, key: &str) -> Option<&Template> {
        let mut locale = self.selected.as_str();
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(locale) {
                return None;
            }
            if let Some(template) = self
                .external
                .get(locale)
                .and_then(|catalog| catalog.messages.get(key))
            {
                return Some(template);
            }
            if let Some(template) = self
                .builtins
                .get(locale)
                .and_then(|catalog| catalog.messages.get(key))
            {
                return Some(template);
            }
            locale = self.fallback_for(locale)?;
        }
    }

    fn fallback_for(&self, locale: &str) -> Option<&str> {
        if let Some(builtin) = self.builtins.get(locale) {
            return builtin.fallback.as_deref();
        }
        if let Some(external) = self.external.get(locale) {
            return Some(external.fallback.as_deref().unwrap_or(FALLBACK_LOCALE));
        }
        (locale != FALLBACK_LOCALE).then_some(FALLBACK_LOCALE)
    }
}

#[derive(Debug, Clone)]
struct Catalog {
    locale: String,
    name: String,
    fallback: Option<String>,
    messages: BTreeMap<String, Template>,
}

type Canonical = BTreeMap<String, BTreeSet<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadMode {
    Builtin,
    Discovered,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Text(String),
    Placeholder(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Template {
    tokens: Vec<Token>,
    placeholders: BTreeSet<String>,
}

impl Template {
    fn render(&self, parameters: &BTreeMap<String, String>) -> String {
        let mut output = String::new();
        for token in &self.tokens {
            match token {
                Token::Text(text) => output.push_str(text),
                Token::Placeholder(name) => match parameters.get(name) {
                    Some(value) => output.push_str(value),
                    None => {
                        output.push('{');
                        output.push_str(name);
                        output.push('}');
                    }
                },
            }
        }
        output
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TemplateError {
    UnclosedPlaceholder,
    UnexpectedClosingBrace,
    EmptyPlaceholder,
    InvalidPlaceholderName(String),
}

impl fmt::Display for TemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnclosedPlaceholder => formatter.write_str("unclosed placeholder"),
            Self::UnexpectedClosingBrace => formatter.write_str("unexpected closing brace"),
            Self::EmptyPlaceholder => formatter.write_str("empty placeholder"),
            Self::InvalidPlaceholderName(name) => {
                write!(formatter, "invalid placeholder name `{name}`")
            }
        }
    }
}

#[derive(Debug)]
enum RawError {
    Parse(String),
    Missing(&'static str),
    SchemaVersion(String),
    LocaleInvalid(String),
    FallbackInvalid(String),
    NameInvalid,
    MessagesNotTable,
    ValueType { key: String, kind: &'static str },
    UnknownTopLevelKey(String),
    UnknownMessageKey(String),
}

impl fmt::Display for RawError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(detail) => formatter.write_str(detail),
            Self::Missing(key) => write!(formatter, "missing required key `{key}`"),
            Self::SchemaVersion(actual) => write!(
                formatter,
                "unsupported schema_version `{actual}` (expected {SCHEMA_VERSION})"
            ),
            Self::LocaleInvalid(locale) => {
                write!(formatter, "invalid locale identifier `{locale}`")
            }
            Self::FallbackInvalid(fallback) => {
                write!(formatter, "invalid fallback locale `{fallback}`")
            }
            Self::NameInvalid => formatter.write_str("`name` must be a non-empty string"),
            Self::MessagesNotTable => formatter.write_str("`messages` must be a table"),
            Self::ValueType { key, kind } => {
                write!(formatter, "message `{key}` must be a string, found {kind}")
            }
            Self::UnknownTopLevelKey(key) => {
                write!(formatter, "unknown top-level key `{key}`")
            }
            Self::UnknownMessageKey(key) => write!(formatter, "unknown message key `{key}`"),
        }
    }
}

#[derive(Debug)]
enum MessageProblem {
    Template(TemplateError),
    PlaceholderMismatch { expected: String, actual: String },
}

impl fmt::Display for MessageProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Template(error) => error.fmt(formatter),
            Self::PlaceholderMismatch { expected, actual } => write!(
                formatter,
                "placeholder mismatch (expected [{expected}], found [{actual}])"
            ),
        }
    }
}

#[derive(Debug)]
enum CatalogError {
    Raw(RawError),
    LocaleMismatch {
        declared: String,
        expected: String,
    },
    Message {
        key: String,
        problem: MessageProblem,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw(error) => error.fmt(formatter),
            Self::LocaleMismatch { declared, expected } => write!(
                formatter,
                "declared locale `{declared}` does not match expected `{expected}`"
            ),
            Self::Message { key, problem } => {
                write!(formatter, "message `{key}`: {problem}")
            }
        }
    }
}

struct RawCatalog {
    locale: String,
    name: String,
    fallback: Option<String>,
    messages: BTreeMap<String, String>,
    unknown_top_level: Vec<String>,
}

fn parse_template(input: &str) -> std::result::Result<Template, TemplateError> {
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut placeholders = BTreeSet::new();
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '{' => {
                if characters.peek() == Some(&'{') {
                    characters.next();
                    text.push('{');
                    continue;
                }
                let mut name = String::new();
                let mut closed = false;
                for character in characters.by_ref() {
                    match character {
                        '}' => {
                            closed = true;
                            break;
                        }
                        '{' => {
                            return Err(TemplateError::InvalidPlaceholderName(name));
                        }
                        _ => name.push(character),
                    }
                }
                if !closed {
                    return Err(TemplateError::UnclosedPlaceholder);
                }
                if name.is_empty() {
                    return Err(TemplateError::EmptyPlaceholder);
                }
                if !is_placeholder_name(&name) {
                    return Err(TemplateError::InvalidPlaceholderName(name));
                }
                if !text.is_empty() {
                    tokens.push(Token::Text(std::mem::take(&mut text)));
                }
                placeholders.insert(name.clone());
                tokens.push(Token::Placeholder(name));
            }
            '}' => {
                if characters.peek() == Some(&'}') {
                    characters.next();
                    text.push('}');
                } else {
                    return Err(TemplateError::UnexpectedClosingBrace);
                }
            }
            _ => text.push(character),
        }
    }
    if !text.is_empty() {
        tokens.push(Token::Text(text));
    }
    Ok(Template {
        tokens,
        placeholders,
    })
}

fn is_placeholder_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn is_valid_locale(locale: &str) -> bool {
    if locale.is_empty() || locale.len() > 64 {
        return false;
    }
    locale.split('-').all(|part| {
        !part.is_empty() && part.len() <= 8 && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn validate_locale(locale: &str) -> I18nResult<()> {
    if is_valid_locale(locale) {
        Ok(())
    } else {
        Err(LocalizedMessage::new("i18n.error.invalid_locale").with_param("locale", locale))
    }
}

fn is_builtin_locale(locale: &str) -> bool {
    BUILT_IN_LOCALES.contains(&locale)
}

fn value_kind(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

fn parse_document(text: &str) -> std::result::Result<RawCatalog, RawError> {
    let table: toml::Table = text
        .parse()
        .map_err(|error: toml::de::Error| RawError::Parse(error.to_string()))?;

    let schema = table
        .get("schema_version")
        .ok_or(RawError::Missing("schema_version"))?;
    match schema.as_integer() {
        Some(version) if version == i64::from(SCHEMA_VERSION) => {}
        Some(version) => return Err(RawError::SchemaVersion(version.to_string())),
        None => {
            return Err(RawError::SchemaVersion(value_kind(schema).to_string()));
        }
    }

    let locale_value = table.get("locale").ok_or(RawError::Missing("locale"))?;
    let Some(locale) = locale_value.as_str() else {
        return Err(RawError::LocaleInvalid(format!(
            "<{}>",
            value_kind(locale_value)
        )));
    };
    if !is_valid_locale(locale) {
        return Err(RawError::LocaleInvalid(locale.to_string()));
    }

    let name_value = table.get("name").ok_or(RawError::Missing("name"))?;
    let Some(name) = name_value.as_str() else {
        return Err(RawError::NameInvalid);
    };
    if name.trim().is_empty() {
        return Err(RawError::NameInvalid);
    }

    let fallback = match table.get("fallback") {
        None => None,
        Some(value) => {
            let Some(fallback) = value.as_str() else {
                return Err(RawError::FallbackInvalid(format!(
                    "<{}>",
                    value_kind(value)
                )));
            };
            if !is_valid_locale(fallback) {
                return Err(RawError::FallbackInvalid(fallback.to_string()));
            }
            Some(fallback.to_string())
        }
    };

    let messages_table = table
        .get("messages")
        .ok_or(RawError::Missing("messages"))?
        .as_table()
        .ok_or(RawError::MessagesNotTable)?;
    let mut messages = BTreeMap::new();
    for (key, value) in messages_table.iter() {
        let Some(message) = value.as_str() else {
            return Err(RawError::ValueType {
                key: key.clone(),
                kind: value_kind(value),
            });
        };
        messages.insert(key.clone(), message.to_string());
    }

    let mut unknown_top_level: Vec<String> = table
        .keys()
        .filter(|key| !TOP_LEVEL_KEYS.contains(&key.as_str()))
        .cloned()
        .collect();
    unknown_top_level.sort();

    Ok(RawCatalog {
        locale: locale.to_string(),
        name: name.to_string(),
        fallback,
        messages,
        unknown_top_level,
    })
}

fn parse_catalog(
    text: &str,
    expected_locale: Option<&str>,
    canonical: Option<&Canonical>,
    mode: LoadMode,
    path: Option<&Path>,
    warnings: &mut Vec<Warning>,
) -> std::result::Result<Catalog, CatalogError> {
    let raw = parse_document(text).map_err(CatalogError::Raw)?;

    for key in raw.unknown_top_level {
        match mode {
            LoadMode::Builtin => {
                return Err(CatalogError::Raw(RawError::UnknownTopLevelKey(key)));
            }
            LoadMode::Discovered | LoadMode::Explicit => push_warning(
                warnings,
                WarningKind::UnknownTopLevelKey,
                path,
                Some(&raw.locale),
                "i18n.warning.unknown_top_level_key",
                &[("path", &path_text(path)), ("key", &key)],
            ),
        }
    }

    if let Some(expected) = expected_locale
        && raw.locale != expected
    {
        return Err(CatalogError::LocaleMismatch {
            declared: raw.locale,
            expected: expected.to_string(),
        });
    }

    if let Some(fallback) = &raw.fallback
        && (!is_builtin_locale(fallback) || fallback == &raw.locale)
    {
        return Err(CatalogError::Raw(RawError::FallbackInvalid(
            fallback.clone(),
        )));
    }

    let mut messages = BTreeMap::new();
    for (key, message) in raw.messages {
        let template = match parse_template(&message) {
            Ok(template) => template,
            Err(error) => match mode {
                LoadMode::Discovered => {
                    push_warning(
                        warnings,
                        WarningKind::InvalidPlaceholder,
                        path,
                        Some(&raw.locale),
                        "i18n.warning.placeholder_invalid",
                        &[
                            ("path", &path_text(path)),
                            ("key", &key),
                            ("detail", &error.to_string()),
                        ],
                    );
                    continue;
                }
                LoadMode::Explicit | LoadMode::Builtin => {
                    return Err(CatalogError::Message {
                        key,
                        problem: MessageProblem::Template(error),
                    });
                }
            },
        };

        if let Some(canonical) = canonical {
            match canonical.get(&key) {
                None => match mode {
                    LoadMode::Builtin => {
                        return Err(CatalogError::Raw(RawError::UnknownMessageKey(key)));
                    }
                    LoadMode::Discovered | LoadMode::Explicit => {
                        push_warning(
                            warnings,
                            WarningKind::UnknownKey,
                            path,
                            Some(&raw.locale),
                            "i18n.warning.unknown_key",
                            &[("path", &path_text(path)), ("key", &key)],
                        );
                        continue;
                    }
                },
                Some(expected) if &template.placeholders != expected => {
                    let expected_text = placeholder_list(expected);
                    let actual_text = placeholder_list(&template.placeholders);
                    match mode {
                        LoadMode::Discovered => {
                            push_warning(
                                warnings,
                                WarningKind::PlaceholderMismatch,
                                path,
                                Some(&raw.locale),
                                "i18n.warning.placeholder_mismatch",
                                &[
                                    ("path", &path_text(path)),
                                    ("key", &key),
                                    ("expected", &expected_text),
                                    ("actual", &actual_text),
                                ],
                            );
                            continue;
                        }
                        LoadMode::Explicit | LoadMode::Builtin => {
                            return Err(CatalogError::Message {
                                key,
                                problem: MessageProblem::PlaceholderMismatch {
                                    expected: expected_text,
                                    actual: actual_text,
                                },
                            });
                        }
                    }
                }
                Some(_) => {}
            }
        }

        messages.insert(key, template);
    }

    Ok(Catalog {
        locale: raw.locale,
        name: raw.name,
        fallback: raw.fallback,
        messages,
    })
}

fn load_builtins() -> I18nResult<BTreeMap<String, Catalog>> {
    let mut warnings = Vec::new();
    let english = parse_catalog(
        EN_US_SOURCE,
        Some("en-US"),
        None,
        LoadMode::Builtin,
        None,
        &mut warnings,
    )
    .map_err(|error| builtin_error("en-US", &error))?;
    let canonical = canonical_placeholders_of(&english);
    let chinese = parse_catalog(
        ZH_CN_SOURCE,
        Some("zh-CN"),
        Some(&canonical),
        LoadMode::Builtin,
        None,
        &mut warnings,
    )
    .map_err(|error| builtin_error("zh-CN", &error))?;
    let mut catalogs = BTreeMap::new();
    catalogs.insert(english.locale.clone(), english);
    catalogs.insert(chinese.locale.clone(), chinese);
    Ok(catalogs)
}

fn canonical_placeholders(catalogs: &BTreeMap<String, Catalog>) -> Canonical {
    catalogs
        .get(FALLBACK_LOCALE)
        .map(canonical_placeholders_of)
        .unwrap_or_default()
}

fn canonical_placeholders_of(catalog: &Catalog) -> Canonical {
    catalog
        .messages
        .iter()
        .map(|(key, template)| (key.clone(), template.placeholders.clone()))
        .collect()
}

fn discover_locales(
    directory: &Path,
    canonical: &Canonical,
    external: &mut BTreeMap<String, Catalog>,
    warnings: &mut Vec<Warning>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            push_warning(
                warnings,
                WarningKind::DirectoryUnreadable,
                Some(directory),
                None,
                "i18n.warning.directory_unreadable",
                &[
                    ("path", &path_text(Some(directory))),
                    ("error", &error.to_string()),
                ],
            );
            return;
        }
    };

    let mut files = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                push_warning(
                    warnings,
                    WarningKind::DirectoryUnreadable,
                    Some(directory),
                    None,
                    "i18n.warning.directory_unreadable",
                    &[
                        ("path", &path_text(Some(directory))),
                        ("error", &error.to_string()),
                    ],
                );
                continue;
            }
        };
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"))
        {
            files.push(path);
        }
    }
    files.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());

    for path in files {
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            push_warning(
                warnings,
                WarningKind::LocaleInvalid,
                Some(&path),
                None,
                "i18n.warning.locale_invalid",
                &[("path", &path_text(Some(&path))), ("locale", "<non-UTF-8>")],
            );
            continue;
        };
        if !is_valid_locale(stem) {
            push_warning(
                warnings,
                WarningKind::LocaleInvalid,
                Some(&path),
                None,
                "i18n.warning.locale_invalid",
                &[("path", &path_text(Some(&path))), ("locale", stem)],
            );
            continue;
        }

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                push_warning(
                    warnings,
                    WarningKind::FileUnreadable,
                    Some(&path),
                    Some(stem),
                    "i18n.warning.file_unreadable",
                    &[
                        ("path", &path_text(Some(&path))),
                        ("error", &error.to_string()),
                    ],
                );
                continue;
            }
        };

        match parse_catalog(
            &text,
            Some(stem),
            Some(canonical),
            LoadMode::Discovered,
            Some(&path),
            warnings,
        ) {
            Ok(catalog) => {
                if external.contains_key(&catalog.locale) {
                    push_warning(
                        warnings,
                        WarningKind::DuplicateLocale,
                        Some(&path),
                        Some(&catalog.locale),
                        "i18n.warning.duplicate_locale",
                        &[
                            ("path", &path_text(Some(&path))),
                            ("locale", &catalog.locale),
                        ],
                    );
                } else {
                    external.insert(catalog.locale.clone(), catalog);
                }
            }
            Err(CatalogError::LocaleMismatch { declared, expected }) => {
                push_warning(
                    warnings,
                    WarningKind::LocaleMismatch,
                    Some(&path),
                    Some(stem),
                    "i18n.warning.locale_mismatch",
                    &[
                        ("path", &path_text(Some(&path))),
                        ("declared", &declared),
                        ("expected", &expected),
                    ],
                );
            }
            Err(CatalogError::Raw(error)) => {
                warn_raw_error(warnings, Some(&path), Some(stem), error);
            }
            Err(CatalogError::Message { key, problem }) => {
                // Discovered message problems are handled above; this is a
                // defensive fallback that keeps the app running.
                push_warning(
                    warnings,
                    WarningKind::MalformedPack,
                    Some(&path),
                    Some(stem),
                    "i18n.warning.malformed_pack",
                    &[
                        ("path", &path_text(Some(&path))),
                        ("error", &format!("message `{key}`: {problem}")),
                    ],
                );
            }
        }
    }
}

fn warn_raw_error(
    warnings: &mut Vec<Warning>,
    path: Option<&Path>,
    locale: Option<&str>,
    error: RawError,
) {
    let path_value = path_text(path);
    match error {
        RawError::SchemaVersion(actual) => push_warning(
            warnings,
            WarningKind::SchemaVersion,
            path,
            locale,
            "i18n.warning.schema_version",
            &[
                ("path", &path_value),
                ("expected", &SCHEMA_VERSION.to_string()),
                ("actual", &actual),
            ],
        ),
        RawError::LocaleInvalid(invalid) => push_warning(
            warnings,
            WarningKind::LocaleInvalid,
            path,
            locale,
            "i18n.warning.locale_invalid",
            &[("path", &path_value), ("locale", &invalid)],
        ),
        RawError::FallbackInvalid(fallback) => push_warning(
            warnings,
            WarningKind::FallbackInvalid,
            path,
            locale,
            "i18n.warning.fallback_invalid",
            &[
                ("path", &path_value),
                ("fallback", &fallback),
                ("allowed", &BUILT_IN_LOCALES.join(", ")),
            ],
        ),
        RawError::ValueType { key, kind } => push_warning(
            warnings,
            WarningKind::ValueType,
            path,
            locale,
            "i18n.warning.value_type",
            &[("path", &path_value), ("key", &key), ("kind", kind)],
        ),
        other => push_warning(
            warnings,
            WarningKind::MalformedPack,
            path,
            locale,
            "i18n.warning.malformed_pack",
            &[("path", &path_value), ("error", &other.to_string())],
        ),
    }
}

fn load_explicit(
    path: &Path,
    selected: &str,
    canonical: &Canonical,
    warnings: &mut Vec<Warning>,
) -> I18nResult<Catalog> {
    let text = fs::read_to_string(path).map_err(|error| unreadable_file(path, error))?;
    let catalog = parse_catalog(
        &text,
        None,
        Some(canonical),
        LoadMode::Explicit,
        Some(path),
        warnings,
    )
    .map_err(|error| invalid_file(path, error))?;
    if catalog.locale != selected {
        return Err(LocalizedMessage::new("i18n.error.locale_mismatch")
            .with_param("path", path.display())
            .with_param("declared", &catalog.locale)
            .with_param("selected", selected));
    }
    Ok(catalog)
}

fn declared_locale(text: &str) -> std::result::Result<String, String> {
    let table: toml::Table = text
        .parse()
        .map_err(|error: toml::de::Error| error.to_string())?;
    let locale = table
        .get("locale")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "key `locale` must be a string".to_string())?;
    if !is_valid_locale(locale) {
        return Err(format!("invalid locale identifier `{locale}`"));
    }
    Ok(locale.to_string())
}

fn unreadable_file(path: &Path, error: impl fmt::Display) -> LocalizedMessage {
    LocalizedMessage::new("i18n.error.file_unreadable")
        .with_param("path", path.display())
        .with_param("error", error)
}

fn invalid_file(path: &Path, detail: impl fmt::Display) -> LocalizedMessage {
    LocalizedMessage::new("i18n.error.invalid_file")
        .with_param("path", path.display())
        .with_param("detail", detail)
}

fn builtin_error(locale: &str, error: &CatalogError) -> LocalizedMessage {
    LocalizedMessage::new("i18n.error.builtin_catalog")
        .with_param("locale", locale)
        .with_param("detail", error)
}

fn push_warning(
    warnings: &mut Vec<Warning>,
    kind: WarningKind,
    path: Option<&Path>,
    locale: Option<&str>,
    key: &str,
    parameters: &[(&str, &str)],
) {
    let mut message = LocalizedMessage::new(key);
    for (name, value) in parameters {
        message = message.with_param(*name, *value);
    }
    warnings.push(Warning {
        kind,
        path: path.map(Path::to_path_buf),
        locale: locale.map(str::to_string),
        message,
    });
}

fn path_text(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn placeholder_list(placeholders: &BTreeSet<String>) -> String {
    if placeholders.is_empty() {
        "(none)".to_string()
    } else {
        placeholders.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// Platforms where the executable has no parent directory (rare) simply have
/// no default discovery location.
fn default_locales_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(|parent| parent.join("locales")))
}

fn legacy_messages() -> &'static BTreeMap<String, Template> {
    static MESSAGES: OnceLock<BTreeMap<String, Template>> = OnceLock::new();
    MESSAGES.get_or_init(|| parse_legacy_messages(ZH_CN_SOURCE).unwrap_or_default())
}

fn parse_legacy_messages(text: &str) -> Option<BTreeMap<String, Template>> {
    let table: toml::Table = text.parse().ok()?;
    let messages = table.get("messages")?.as_table()?;
    let mut parsed = BTreeMap::new();
    for (key, value) in messages {
        if let Some(message) = value.as_str()
            && let Ok(template) = parse_template(message)
        {
            parsed.insert(key.clone(), template);
        }
    }
    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error as IoError, ErrorKind};
    use tempfile::TempDir;

    fn write_locales(directory: &TempDir, files: &[(&str, &str)]) {
        for (name, contents) in files {
            fs::write(directory.path().join(name), contents).unwrap();
        }
    }

    fn discover_at(directory: &TempDir, selected: &str) -> I18n {
        I18n::load(
            selected,
            &LoadOptions::default().with_locales_dir(directory.path()),
        )
        .unwrap()
    }

    fn catalog(locale: &str, name: &str, fallback: Option<&str>, messages: &str) -> String {
        let fallback = fallback
            .map(|fallback| format!("fallback = \"{fallback}\"\n"))
            .unwrap_or_default();
        format!(
            "schema_version = 1\nlocale = \"{locale}\"\nname = \"{name}\"\n{fallback}\n[messages]\n{messages}\n"
        )
    }

    #[test]
    fn builtin_catalogs_have_identical_keys_and_placeholders() {
        let builtins = load_builtins().unwrap();
        let english = &builtins[FALLBACK_LOCALE];
        let chinese = &builtins[DEFAULT_LOCALE];
        assert_eq!(
            english.messages.keys().collect::<Vec<_>>(),
            chinese.messages.keys().collect::<Vec<_>>()
        );
        for (key, template) in &english.messages {
            assert_eq!(
                template.placeholders, chinese.messages[key].placeholders,
                "placeholder mismatch for {key}"
            );
        }
        assert!(!english.messages.is_empty());
    }

    #[test]
    fn default_service_is_chinese_with_english_fallback() {
        let service = I18n::default();
        assert_eq!(service.selected_locale(), DEFAULT_LOCALE);
        assert_eq!(service.translate("status.trusted"), "可信");
        assert_eq!(service.translate("missing.key"), "missing.key");
        assert!(service.warnings().is_empty());

        let english = service.select(FALLBACK_LOCALE).unwrap();
        assert_eq!(english.translate("status.trusted"), "Trusted");
        assert_eq!(
            english.translate_with("gui.progress.files", &[("completed", "2"), ("total", "5")]),
            "2 / 5 files"
        );
    }

    #[test]
    fn escaped_braces_are_literal() {
        let template = parse_template("{{name}} and {{{value}}}").unwrap();
        assert!(template.placeholders.contains("value"));
        assert!(!template.placeholders.contains("name"));
        let mut values = BTreeMap::new();
        values.insert("value".to_string(), "7".to_string());
        assert_eq!(template.render(&values), "{name} and {7}");

        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                &catalog(
                    "zh-CN",
                    "自定义",
                    Some("en-US"),
                    "\"status.trusted\" = \"{{literal}}\"",
                ),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.translate("status.trusted"), "{literal}");
        assert!(service.warnings().is_empty());
    }

    #[test]
    fn external_messages_override_builtin_selected_locale() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                &catalog(
                    "zh-CN",
                    "自定义中文",
                    Some("en-US"),
                    "\"status.trusted\" = \"自定义可信\"",
                ),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert!(service.warnings().is_empty());
        assert_eq!(service.translate("status.trusted"), "自定义可信");
        // Missing messages still come from the built-in selected catalog.
        assert_eq!(service.translate("status.needs_review"), "需人工确认");
        let english = service.select(FALLBACK_LOCALE).unwrap();
        assert_eq!(english.translate("status.trusted"), "Trusted");
        let languages = service.available_languages();
        let chinese = languages
            .iter()
            .find(|language| language.id == "zh-CN")
            .unwrap();
        assert_eq!(chinese.name, "自定义中文");
        assert!(chinese.built_in);
    }

    #[test]
    fn new_locale_merges_over_declared_fallback() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[
                (
                    "fr-FR.toml",
                    &catalog(
                        "fr-FR",
                        "Français",
                        Some("zh-CN"),
                        "\"status.trusted\" = \"Fiable\"",
                    ),
                ),
                (
                    "de-DE.toml",
                    &catalog(
                        "de-DE",
                        "Deutsch",
                        None,
                        "\"status.trusted\" = \"Vertrauenswürdig\"",
                    ),
                ),
            ],
        );
        let french = discover_at(&directory, "fr-FR");
        assert_eq!(french.translate("status.trusted"), "Fiable");
        assert_eq!(french.translate("status.needs_review"), "需人工确认");
        assert_eq!(
            french.translate("detection.reason.consistent"),
            "多条分离直线方向一致"
        );
        assert!(french.warnings().is_empty());

        let german = discover_at(&directory, "de-DE");
        assert_eq!(german.translate("status.trusted"), "Vertrauenswürdig");
        assert_eq!(german.translate("status.needs_review"), "Needs review");

        let languages = french.available_languages();
        assert_eq!(
            languages
                .iter()
                .find(|language| language.id == "fr-FR")
                .unwrap()
                .name,
            "Français"
        );
        assert!(
            !languages
                .iter()
                .find(|language| language.id == "fr-FR")
                .unwrap()
                .built_in
        );
    }

    #[test]
    fn discovered_malformed_pack_warns_and_skips() {
        let directory = TempDir::new().unwrap();
        write_locales(&directory, &[("zh-CN.toml", "this is not toml [")]);
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::MalformedPack);
        assert_eq!(service.translate("status.trusted"), "可信");
        let text = service.warning_texts();
        assert!(text[0].contains("已跳过"), "{text:?}");
        let english = service.select(FALLBACK_LOCALE).unwrap();
        assert!(english.warning_texts()[0].contains("skipped"));
    }

    #[test]
    fn explicit_malformed_file_is_an_error() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("custom.toml");
        fs::write(&path, "not toml [").unwrap();
        let error =
            I18n::load("zh-CN", &LoadOptions::default().with_language_file(&path)).unwrap_err();
        assert_eq!(error.key(), "i18n.error.invalid_file");
        assert!(error.to_string().contains("custom.toml"));
    }

    #[test]
    fn unsupported_schema_version_warns_or_fails() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                "schema_version = 2\nlocale = \"zh-CN\"\nname = \"x\"\n[messages]\n",
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::SchemaVersion);
        assert_eq!(service.translate("status.trusted"), "可信");

        let path = directory.path().join("zh-CN.toml");
        let error =
            I18n::load("zh-CN", &LoadOptions::default().with_language_file(&path)).unwrap_err();
        assert_eq!(error.key(), "i18n.error.invalid_file");
        assert!(error.to_string().contains("schema_version"));
    }

    #[test]
    fn non_string_message_value_warns_or_fails() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                "schema_version = 1\nlocale = \"zh-CN\"\nname = \"x\"\n[messages]\n\"status.trusted\" = 42\n",
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::ValueType);
        assert_eq!(service.translate("status.trusted"), "可信");

        let path = directory.path().join("zh-CN.toml");
        let error =
            I18n::load("zh-CN", &LoadOptions::default().with_language_file(&path)).unwrap_err();
        assert_eq!(error.key(), "i18n.error.invalid_file");
        assert!(error.to_string().contains("string"));
    }

    #[test]
    fn duplicate_keys_warn_or_fail() {
        let duplicate = "schema_version = 1\nlocale = \"zh-CN\"\nname = \"x\"\n[messages]\n\"status.trusted\" = \"a\"\n\"status.trusted\" = \"b\"\n";
        let directory = TempDir::new().unwrap();
        write_locales(&directory, &[("zh-CN.toml", duplicate)]);
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::MalformedPack);
        assert_eq!(service.translate("status.trusted"), "可信");

        let path = directory.path().join("zh-CN.toml");
        let error =
            I18n::load("zh-CN", &LoadOptions::default().with_language_file(&path)).unwrap_err();
        assert_eq!(error.key(), "i18n.error.invalid_file");
    }

    #[test]
    fn invalid_placeholders_warn_and_are_ignored_or_fail_explicitly() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                &catalog(
                    "zh-CN",
                    "x",
                    Some("en-US"),
                    "\"imaging.error.pixel_limit\" = \"缺少占位符 {width}\"\n\"status.trusted\" = \"有效\"\n\"status.needs_review\" = \"坏 {bad name}\"",
                ),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        let kinds: Vec<_> = service.warnings().iter().map(Warning::kind).collect();
        assert!(kinds.contains(&WarningKind::PlaceholderMismatch));
        assert!(kinds.contains(&WarningKind::InvalidPlaceholder));
        // The valid neighboring message is still applied.
        assert_eq!(service.translate("status.trusted"), "有效");
        // Invalid messages keep the built-in translation.
        assert!(
            service
                .translate("imaging.error.pixel_limit")
                .contains("{width}")
        );
        assert_eq!(service.translate("status.needs_review"), "需人工确认");

        let path = directory.path().join("zh-CN.toml");
        let error =
            I18n::load("zh-CN", &LoadOptions::default().with_language_file(&path)).unwrap_err();
        assert_eq!(error.key(), "i18n.error.invalid_file");
        assert!(error.to_string().contains("placeholder"));
    }

    #[test]
    fn unmatched_braces_are_rejected_by_the_template_parser() {
        assert_eq!(
            parse_template("value {"),
            Err(TemplateError::UnclosedPlaceholder)
        );
        assert_eq!(
            parse_template("value }"),
            Err(TemplateError::UnexpectedClosingBrace)
        );
        assert_eq!(parse_template("{}"), Err(TemplateError::EmptyPlaceholder));
        assert!(matches!(
            parse_template("{bad name}"),
            Err(TemplateError::InvalidPlaceholderName(_))
        ));
        assert!(matches!(
            parse_template("{foo + 1}"),
            Err(TemplateError::InvalidPlaceholderName(_))
        ));
    }

    #[test]
    fn unknown_keys_warn_and_are_ignored() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                &catalog(
                    "zh-CN",
                    "x",
                    Some("en-US"),
                    "\"status.trusted\" = \"可信2\"\n\"not.a.real.key\" = \"ignored\"",
                ),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::UnknownKey);
        assert_eq!(service.translate("status.trusted"), "可信2");
        assert_eq!(service.translate("not.a.real.key"), "not.a.real.key");
    }

    #[test]
    fn unknown_top_level_keys_warn_and_are_ignored() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                &format!(
                    "extra = true\n{}",
                    catalog(
                        "zh-CN",
                        "x",
                        Some("en-US"),
                        "\"status.trusted\" = \"可信2\""
                    )
                ),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(
            service.warnings()[0].kind(),
            WarningKind::UnknownTopLevelKey
        );
        assert_eq!(service.translate("status.trusted"), "可信2");
    }

    #[test]
    fn locale_identifiers_are_validated_against_traversal() {
        for locale in ["../evil", "a/b", "a\\b", ".", "..", "", "en--US", "-en"] {
            assert!(I18n::with_locale(locale).is_err(), "{locale}");
            assert!(!is_valid_locale(locale), "{locale}");
        }
        for locale in ["zh-CN", "en-US", "fr", "zh-Hans-CN", "pt-BR", "x1"] {
            assert!(is_valid_locale(locale), "{locale}");
        }

        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[("bad.name.toml", &catalog("bad.name", "x", None, ""))],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::LocaleInvalid);
    }

    #[test]
    fn declared_locale_must_match_file_name() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "zh-CN.toml",
                &catalog("en-US", "x", None, "\"status.trusted\" = \"x\""),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::LocaleMismatch);
        assert_eq!(service.translate("status.trusted"), "可信");
    }

    #[test]
    fn invalid_fallback_warns_or_fails() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "fr-FR.toml",
                &catalog("fr-FR", "Français", Some("xx-YY"), ""),
            )],
        );
        let service = discover_at(&directory, "fr-FR");
        assert_eq!(service.warnings().len(), 1);
        assert_eq!(service.warnings()[0].kind(), WarningKind::FallbackInvalid);
        assert_eq!(service.translate("status.trusted"), "Trusted");

        let path = directory.path().join("fr-FR.toml");
        let error = I18n::from_language_file(&path).unwrap_err();
        assert_eq!(error.key(), "i18n.error.invalid_file");
    }

    #[test]
    fn explicit_file_locale_must_match_selected_locale() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("fr-FR.toml");
        fs::write(
            &path,
            catalog(
                "fr-FR",
                "Français",
                Some("en-US"),
                "\"status.trusted\" = \"Fiable\"",
            ),
        )
        .unwrap();
        let error =
            I18n::load("zh-CN", &LoadOptions::default().with_language_file(&path)).unwrap_err();
        assert_eq!(error.key(), "i18n.error.locale_mismatch");

        let service = I18n::from_language_file(&path).unwrap();
        assert_eq!(service.selected_locale(), "fr-FR");
        assert_eq!(service.translate("status.trusted"), "Fiable");
        assert_eq!(service.translate("status.needs_review"), "Needs review");
    }

    #[test]
    fn translated_anyhow_chain_preserves_unknown_details() {
        let localized =
            LocalizedMessage::new("gui.error.open_window").with_param("error", "native detail");
        let error = AnyhowError::new(localized).context(
            IoError::new(
                ErrorKind::NotFound,
                "The system cannot find the file specified.",
            )
            .to_string(),
        );
        let chinese = I18n::default();
        let rendered = chinese.render_anyhow(&error);
        assert!(
            rendered.contains("无法打开窗口：native detail"),
            "{rendered}"
        );
        assert!(
            rendered.contains("The system cannot find the file specified."),
            "{rendered}"
        );

        let english = chinese.select(FALLBACK_LOCALE).unwrap();
        let rendered = english.render_anyhow(&error);
        assert!(
            rendered.contains("Unable to open the window: native detail"),
            "{rendered}"
        );
        assert!(rendered.contains("The system cannot find the file specified."));

        let direct = AnyhowError::new(
            LocalizedMessage::new("gui.error.open_window").with_param("error", "native detail"),
        );
        assert_eq!(
            english.render_anyhow(&direct),
            "Unable to open the window: native detail"
        );
    }

    #[test]
    fn localized_message_display_defaults_to_chinese() {
        let message = LocalizedMessage::new("status.trusted");
        assert_eq!(message.to_string(), "可信");
        let message = LocalizedMessage::new("gui.progress.files")
            .with_param("completed", 3)
            .with_param("total", 9);
        assert_eq!(message.to_string(), "3 / 9 文件");
        assert_eq!(
            LocalizedMessage::new("unknown.key").to_string(),
            "unknown.key"
        );
    }

    #[test]
    fn saved_preference_is_applied_only_when_available_and_valid() {
        let directory = TempDir::new().unwrap();
        write_locales(
            &directory,
            &[(
                "fr-FR.toml",
                &catalog(
                    "fr-FR",
                    "Français",
                    Some("en-US"),
                    "\"status.trusted\" = \"Fiable\"",
                ),
            )],
        );
        let service = discover_at(&directory, "zh-CN");
        let french = service.select_saved(Some("fr-FR")).unwrap();
        assert_eq!(french.selected_locale(), "fr-FR");
        assert_eq!(
            service
                .select_saved(Some("missing-Locale"))
                .unwrap()
                .selected_locale(),
            "zh-CN"
        );
        assert_eq!(
            service
                .select_saved(Some("../bad"))
                .unwrap()
                .selected_locale(),
            "zh-CN"
        );
        assert_eq!(
            service.select_saved(None).unwrap().selected_locale(),
            "zh-CN"
        );
    }

    #[test]
    fn discovery_ignores_missing_directory_without_warning() {
        let directory = TempDir::new().unwrap();
        let missing = directory.path().join("does-not-exist");
        let service =
            I18n::load("zh-CN", &LoadOptions::default().with_locales_dir(&missing)).unwrap();
        assert!(service.warnings().is_empty());
        assert_eq!(service.translate("status.trusted"), "可信");
    }

    #[test]
    fn discovery_skips_non_toml_and_directory_entries() {
        let directory = TempDir::new().unwrap();
        fs::write(directory.path().join("notes.txt"), "ignored").unwrap();
        fs::create_dir(directory.path().join("fr-FR.toml")).unwrap();
        let service = discover_at(&directory, "zh-CN");
        assert!(service.warnings().is_empty());
        assert_eq!(service.available_languages().len(), 2);
    }
}
