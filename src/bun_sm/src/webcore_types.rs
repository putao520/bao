// @trace REQ-ENG-001
// WebCore type definitions for SpiderMonkey.
// Maps servo's WebIDL-generated DOM types to Rust surface types,
// providing the type bridge between bao_engine and servo DOM.

/// DOM node types (matches servo's dom::NodeType).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomNodeType {
    Element,
    Attribute,
    Text,
    CDataSection,
    EntityReference,
    Entity,
    ProcessingInstruction,
    Comment,
    Document,
    DocumentType,
    DocumentFragment,
    Notation,
}

/// WebCore event phase (matches DOM Event.eventPhase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPhase {
    None = 0,
    Capturing = 1,
    AtTarget = 2,
    Bubbling = 3,
}

/// WebCore request mode (matches Fetch spec RequestMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    Navigate,
    SameOrigin,
    NoCors,
    Cors,
}

/// WebCore response type (matches Fetch spec ResponseType).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseType {
    Basic,
    Cors,
    Default,
    Error,
    Opaque,
    OpaqueRedirect,
}

/// WebCore redirect mode (matches Fetch spec RequestRedirect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectMode {
    Follow,
    Error,
    Manual,
}

/// WebCore referrer policy (matches Fetch spec ReferrerPolicy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferrerPolicy {
    None,
    NoReferrer,
    NoReferrerWhenDowngrade,
    Origin,
    OriginWhenCrossOrigin,
    SameOrigin,
    StrictOrigin,
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

/// WebCore cache mode (matches Fetch spec RequestCache).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    Default,
    NoStore,
    Reload,
    NoCache,
    ForceCache,
    OnlyIfCached,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dom_node_types() {
        assert_eq!(DomNodeType::Element, DomNodeType::Element);
        assert_ne!(DomNodeType::Element, DomNodeType::Text);
    }

    #[test]
    fn event_phase_values() {
        assert_eq!(EventPhase::None as u8, 0);
        assert_eq!(EventPhase::Capturing as u8, 1);
        assert_eq!(EventPhase::AtTarget as u8, 2);
        assert_eq!(EventPhase::Bubbling as u8, 3);
    }

    #[test]
    fn request_mode_coverage() {
        assert_ne!(RequestMode::Navigate, RequestMode::Cors);
    }

    #[test]
    fn response_type_coverage() {
        assert_eq!(ResponseType::Default, ResponseType::Default);
        assert_ne!(ResponseType::Error, ResponseType::Basic);
    }
}
