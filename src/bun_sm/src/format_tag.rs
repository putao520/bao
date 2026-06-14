//! Format tag type.

#[derive(Debug, Clone, Copy)]
pub enum FormatTag {
    Default,
    JSON,
    Inspect,
}

pub enum FormatAs {
    Default,
    JSON,
    Inspect,
}
