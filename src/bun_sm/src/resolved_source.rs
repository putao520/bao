//! Resolved source — module resolution result.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Esm,
    Cjs,
    Json,
    Wasm,
    Object,
    Unknown,
}

pub struct ResolvedSource<'a> {
    pub specifier: &'a str,
    pub source_url: &'a str,
    pub source_code: &'a str,
    pub tag: Tag,
}

pub struct OwnedResolvedSource {
    pub specifier: String,
    pub source_url: String,
    pub source_code: String,
    pub tag: Tag,
}

impl OwnedResolvedSource {
    pub fn as_resolved_source(&self) -> ResolvedSource<'_> {
        ResolvedSource {
            specifier: &self.specifier,
            source_url: &self.source_url,
            source_code: &self.source_code,
            tag: self.tag,
        }
    }
}

impl<'a> ResolvedSource<'a> {
    pub fn to_owned(&self) -> OwnedResolvedSource {
        OwnedResolvedSource {
            specifier: self.specifier.to_string(),
            source_url: self.source_url.to_string(),
            source_code: self.source_code.to_string(),
            tag: self.tag,
        }
    }
}
