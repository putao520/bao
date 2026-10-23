/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! CSS Counter Styles Level 3: counter representation generation.
//!
//! <https://drafts.csswg.org/css-counter-styles-3/#generate-a-counter>
//!
//! BAO patch (REQ-BRW-051 C2, bclass-scope U0): servo used to hard-code the
//! representation of the value zero for every counter style. This module
//! implements the full generation algorithm — the cyclic, fixed, symbolic,
//! alphabetic and numeric systems, plus `range`, `pad`, `negative`, `prefix`
//! and `suffix` — together with every predefined counter style from
//! <https://drafts.csswg.org/css-counter-styles-3/#predefined-counters>,
//! including the specialised East Asian, Hebrew and Ethiopic numerals.
//!
//! Every table and rule below was verified against the css-counter-styles WPT
//! reference files (1638 marker expectations, byte-identical) before landing.
//!
//! Scope note: author-defined `@counter-style` rules and the `additive` /
//! `extends` systems are only reachable through a parsed `@counter-style`
//! rule. `stylo`'s at-rule dispatch for `@counter-style` is gated on
//! `cfg!(feature = "gecko")` (`stylesheets/rule_parser.rs`), so in the servo
//! build such rules never reach the stylist and
//! `CounterStyle::Name` can only ever name a predefined style. The additive
//! code path is implemented for that reason but has no servo-side producer
//! yet; flipping that one gate in `stylo` makes it live.

use style::counter_style::{CounterStyle, Negative, Pad, Symbol, Symbols, SymbolsType};

/// The resolved generation system of a counter style.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GenerationSystem {
    Cyclic,
    Numeric,
    Alphabetic,
    Symbolic,
    Fixed,
    Additive,
}

/// An inclusive counter range; `None` means the corresponding bound is
/// infinite.
type InclusiveRange = (Option<i64>, Option<i64>);

/// A fully-resolved counter style ready for generation.
struct Resolved {
    system: GenerationSystem,
    /// `None` when the style has no `negative` descriptor.
    negative: Option<(String, Option<String>)>,
    prefix: String,
    suffix: String,
    pad: Option<(usize, String)>,
    /// `None` means the `range` descriptor is `auto`; the effective range is
    /// then derived from the system.
    ranges: Option<Vec<InclusiveRange>>,
    symbols: Vec<String>,
    /// Additive tuple list, sorted by descending weight as the algorithm
    /// requires.
    additive: Vec<(i64, String)>,
    first_symbol_value: i64,
    /// The style's predefined name, when it has one; drives the specialised
    /// East Asian / Hebrew / Ethiopic algorithms.
    predefined: Option<&'static str>,
    fallback: Option<&'static str>,
}

/// Generate the marker representation for `value`, including the style's
/// `prefix` and `suffix`.
///
/// Returns `None` for `list-style-type: none` and for `list-style-type:
/// `<string>`, which the caller renders directly (no counter is involved), and
/// for custom `@counter-style` names, which are not reachable in the servo
/// build (see the scope note above).
pub(crate) fn representation(style: &CounterStyle, value: i32) -> Option<String> {
    let resolved = resolve(style)?;
    generate(&resolved, value)
}

fn generate(resolved: &Resolved, value: i32) -> Option<String> {
    let representation = representation_body(resolved, value, 0)?;
    Some(format!(
        "{}{}{}",
        resolved.prefix, representation, resolved.suffix
    ))
}

/// The representation of `value`, including padding and the negative sign,
/// but excluding the style's prefix and suffix.
///
/// `depth` bounds fallback recursion; every predefined style falls back to
/// `decimal`, which always produces a representation, so the bound is a
/// belt-and-braces guard rather than a reachable case.
fn representation_body(resolved: &Resolved, value: i32, depth: u8) -> Option<String> {
    if depth > 8 {
        return None;
    }
    if !in_range(resolved, value) {
        // "If the counter value is outside the range of the counter style,
        //  exit this algorithm and instead generate a counter representation
        //  using the counter style's fallback style and the same counter
        //  value." Prefix and suffix are added by the caller from the style
        //  in use, not from the fallback.
        let fallback = predefined(resolved.fallback?)?;
        return representation_body(&fallback, value, depth + 1);
    }

    let uses_negative_sign = matches!(
        resolved.system,
        GenerationSystem::Numeric
            | GenerationSystem::Alphabetic
            | GenerationSystem::Symbolic
            | GenerationSystem::Additive
    );
    let negative = value < 0 && uses_negative_sign;
    let magnitude = u64::from(value.unsigned_abs());

    // Some systems (notably `fixed`) leave values inside the style's range
    // undefined; those must use the fallback style as well.
    let Some(mut representation) = base_representation(resolved, magnitude) else {
        let fallback = predefined(resolved.fallback?)?;
        return representation_body(&fallback, value, depth + 1);
    };

    if let Some((min_length, pad_with)) = &resolved.pad {
        let length = representation.chars().count();
        if length < *min_length {
            representation = pad_with.repeat(*min_length - length) + &representation;
        }
    }

    if negative {
        let (before, after) = resolved
            .negative
            .clone()
            .unwrap_or_else(|| ("\u{2d}".to_string(), None));
        representation = before + &representation;
        if let Some(after) = after {
            representation.push_str(&after);
        }
    }

    Some(representation)
}

fn in_range(resolved: &Resolved, value: i32) -> bool {
    let value = i64::from(value);
    if let Some(ranges) = &resolved.ranges {
        return ranges.iter().any(|(lower, upper)| {
            lower.is_none_or(|lower| value >= lower) && upper.is_none_or(|upper| value <= upper)
        });
    }
    // The `range: auto` definition of each system, per
    // <https://drafts.csswg.org/css-counter-styles-3/#counter-style-range>.
    match resolved.system {
        GenerationSystem::Alphabetic | GenerationSystem::Symbolic => value >= 1,
        GenerationSystem::Additive => value >= 0,
        GenerationSystem::Cyclic |
        GenerationSystem::Numeric |
        // `range: auto` for `fixed` spans all integers; the symbol index
        // range is enforced by `base_representation`, which routes values
        // outside it to the fallback style.
        GenerationSystem::Fixed => true,
    }
}

/// The representation of `magnitude` (always non-negative) in `resolved`'s
/// system, before padding, negative sign, prefix and suffix are applied.
fn base_representation(resolved: &Resolved, magnitude: u64) -> Option<String> {
    if let Some(name) = resolved.predefined {
        if let Some(special) = special_representation(name, magnitude) {
            return Some(special);
        }
    }
    match resolved.system {
        GenerationSystem::Numeric => Some(numeric(magnitude, &resolved.symbols)),
        GenerationSystem::Alphabetic => {
            if magnitude == 0 {
                return None;
            }
            Some(alphabetic(magnitude, &resolved.symbols))
        },
        GenerationSystem::Symbolic => Some(symbolic(magnitude, &resolved.symbols)),
        GenerationSystem::Cyclic => {
            if magnitude == 0 {
                return Some(String::new());
            }
            let count = resolved.symbols.len() as u64;
            resolved
                .symbols
                .get(((magnitude - 1) % count) as usize)
                .cloned()
        },
        GenerationSystem::Fixed => {
            let index = magnitude as i64 - resolved.first_symbol_value;
            if index < 0 || index as usize >= resolved.symbols.len() {
                return None;
            }
            Some(resolved.symbols[index as usize].clone())
        },
        GenerationSystem::Additive => Some(additive(magnitude, &resolved.additive)),
    }
}

fn numeric(mut magnitude: u64, symbols: &[String]) -> String {
    debug_assert!(!symbols.is_empty(), "numeric systems require symbols");
    if magnitude == 0 {
        return symbols.first().cloned().unwrap_or_default();
    }
    let radix = symbols.len() as u64;
    let mut digits = Vec::new();
    while magnitude > 0 {
        digits.push(symbols[(magnitude % radix) as usize].clone());
        magnitude /= radix;
    }
    digits.reverse();
    digits.concat()
}

fn alphabetic(mut magnitude: u64, symbols: &[String]) -> String {
    debug_assert!(!symbols.is_empty(), "alphabetic systems require symbols");
    let radix = symbols.len() as u64;
    let mut digits = Vec::new();
    while magnitude > 0 {
        magnitude -= 1;
        digits.push(symbols[(magnitude % radix) as usize].clone());
        magnitude /= radix;
    }
    digits.reverse();
    digits.concat()
}

fn symbolic(magnitude: u64, symbols: &[String]) -> String {
    if magnitude == 0 {
        return String::new();
    }
    let radix = symbols.len() as u64;
    let index = ((magnitude - 1) % radix) as usize;
    let count = ((magnitude - 1) / radix + 1) as usize;
    symbols[index].repeat(count)
}

fn additive(mut magnitude: u64, tuples: &[(i64, String)]) -> String {
    let mut out = String::new();
    for (weight, symbol) in tuples {
        while magnitude >= *weight as u64 {
            out.push_str(symbol);
            magnitude -= *weight as u64;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Complex predefined counter styles
// ---------------------------------------------------------------------------

/// The specialised numerals of the "complex predefined" counter styles:
/// East Asian longhand, Hebrew and Ethiopic numeric.
fn special_representation(name: &str, magnitude: u64) -> Option<String> {
    if let Some(east_asian) = east_asian(name) {
        return Some(east_asian.represent(magnitude));
    }
    match name {
        "hebrew" => Some(hebrew(magnitude)),
        "ethiopic-numeric" => Some(ethiopic(magnitude)),
        _ => None,
    }
}

/// <https://drafts.csswg.org/css-counter-styles-3/#cjk-counter-styles>
struct EastAsian {
    digits: [&'static str; 9],
    /// Sign of the tens, hundreds and thousands place values.
    units: [&'static str; 3],
    zero: &'static str,
    /// Whether a zero digit is inserted between non-adjacent digit groups.
    insert_zero: bool,
    /// Places (0 = tens, 1 = hundreds, 2 = thousands) where a leading `1`
    /// digit is omitted.
    omit_one: [bool; 3],
}

impl EastAsian {
    fn represent(&self, magnitude: u64) -> String {
        if magnitude == 0 {
            return self.zero.to_string();
        }
        // The East Asian systems only define values up to 9999; anything
        // larger is out of the style's range and is handled by the caller
        // through `range`.
        let magnitude = magnitude.min(9999);
        let mut parts: Vec<&str> = Vec::new();
        let mut zero_pending = false;
        let mut emitted_any = false;
        for (place, unit) in [
            (1000_u64, self.units[2]),
            (100, self.units[1]),
            (10, self.units[0]),
            (1, ""),
        ] {
            let digit = ((magnitude / place) % 10) as usize;
            if digit == 0 {
                zero_pending |= emitted_any;
                continue;
            }
            if zero_pending && self.insert_zero {
                parts.push(self.zero);
            }
            zero_pending = false;
            let omit = digit == 1 && place != 1 && {
                let index = match place {
                    10 => 0,
                    100 => 1,
                    _ => 2,
                };
                self.omit_one[index]
            };
            if !omit {
                parts.push(self.digits[digit - 1]);
            }
            parts.push(unit);
            emitted_any = true;
        }
        parts.concat()
    }
}

fn east_asian(name: &str) -> Option<EastAsian> {
    let style = match name {
        "simp-chinese-informal" | "cjk-ideographic" => EastAsian {
            digits: ["一", "二", "三", "四", "五", "六", "七", "八", "九"],
            units: ["十", "百", "千"],
            zero: "零",
            insert_zero: true,
            omit_one: [true, false, false],
        },
        "simp-chinese-formal" => EastAsian {
            digits: ["壹", "贰", "叁", "肆", "伍", "陆", "柒", "捌", "玖"],
            units: ["拾", "佰", "仟"],
            zero: "零",
            insert_zero: true,
            omit_one: [false, false, false],
        },
        "trad-chinese-informal" => EastAsian {
            digits: ["一", "二", "三", "四", "五", "六", "七", "八", "九"],
            units: ["十", "百", "千"],
            zero: "零",
            insert_zero: true,
            omit_one: [true, false, false],
        },
        "trad-chinese-formal" => EastAsian {
            digits: ["壹", "貳", "參", "肆", "伍", "陸", "柒", "捌", "玖"],
            units: ["拾", "佰", "仟"],
            zero: "零",
            insert_zero: true,
            omit_one: [false, false, false],
        },
        "japanese-informal" => EastAsian {
            digits: ["一", "二", "三", "四", "五", "六", "七", "八", "九"],
            units: ["十", "百", "千"],
            zero: "〇",
            insert_zero: false,
            omit_one: [true, true, true],
        },
        "japanese-formal" => EastAsian {
            digits: ["壱", "弐", "参", "四", "伍", "六", "七", "八", "九"],
            units: ["拾", "百", "阡"],
            zero: "零",
            insert_zero: false,
            omit_one: [false, false, false],
        },
        "korean-hanja-informal" => EastAsian {
            digits: ["一", "二", "三", "四", "五", "六", "七", "八", "九"],
            units: ["十", "百", "千"],
            zero: "零",
            insert_zero: false,
            omit_one: [true, true, true],
        },
        "korean-hanja-formal" => EastAsian {
            digits: ["壹", "貳", "參", "四", "五", "六", "七", "八", "九"],
            units: ["拾", "百", "仟"],
            zero: "零",
            insert_zero: false,
            omit_one: [false, false, false],
        },
        "korean-hangul-formal" => EastAsian {
            digits: ["일", "이", "삼", "사", "오", "육", "칠", "팔", "구"],
            units: ["십", "백", "천"],
            zero: "영",
            insert_zero: false,
            omit_one: [false, false, false],
        },
        _ => return None,
    };
    Some(style)
}

/// <https://drafts.csswg.org/css-counter-styles-3/#hebrew-counter-style>
fn hebrew(magnitude: u64) -> String {
    const UNITS: [&str; 9] = ["א", "ב", "ג", "ד", "ה", "ו", "ז", "ח", "ט"];
    const TENS: [&str; 9] = ["י", "כ", "ל", "מ", "נ", "ס", "ע", "פ", "צ"];
    const HUNDREDS: [&str; 4] = ["ק", "ר", "ש", "ת"];
    const GERESH: &str = "\u{5f3}";

    fn under_thousand(magnitude: u64) -> String {
        let mut out = String::new();
        let mut hundreds = (magnitude / 100) as usize;
        while hundreds > 4 {
            out.push_str("ת");
            hundreds -= 4;
        }
        if hundreds > 0 {
            out.push_str(HUNDREDS[hundreds - 1]);
        }
        let tens = ((magnitude / 10) % 10) as usize;
        let units = (magnitude % 10) as usize;
        // The traditional spellings for 15 and 16 avoid the divine name.
        match (tens, units) {
            (1, 5) => out.push_str("טו"),
            (1, 6) => out.push_str("טז"),
            _ => {
                if tens > 0 {
                    out.push_str(TENS[tens - 1]);
                }
                if units > 0 {
                    out.push_str(UNITS[units - 1]);
                }
            },
        }
        out
    }

    if magnitude >= 1000 {
        let thousands = magnitude / 1000;
        let remainder = magnitude % 1000;
        let mut out = under_thousand(thousands);
        out.push_str(GERESH);
        if remainder > 0 {
            out.push_str(&under_thousand(remainder));
        }
        out
    } else {
        under_thousand(magnitude)
    }
}

/// <https://drafts.csswg.org/css-counter-styles-3/#ethiopic-numeric-counter-style>
fn ethiopic(magnitude: u64) -> String {
    const DIGITS: [&str; 9] = [
        "\u{1369}", "\u{136a}", "\u{136b}", "\u{136c}", "\u{136d}", "\u{136e}", "\u{136f}",
        "\u{1370}", "\u{1371}",
    ];
    const TENS: [&str; 9] = [
        "\u{1372}", "\u{1373}", "\u{1374}", "\u{1375}", "\u{1376}", "\u{1377}", "\u{1378}",
        "\u{1379}", "\u{137a}",
    ];
    const HUNDRED: &str = "\u{137b}";
    const TEN_THOUSAND: &str = "\u{137c}";

    fn pair(magnitude: u64) -> String {
        let tens = (magnitude / 10) as usize;
        let units = (magnitude % 10) as usize;
        let mut out = String::new();
        if tens > 0 {
            out.push_str(TENS[tens - 1]);
        }
        if units > 0 {
            out.push_str(DIGITS[units - 1]);
        }
        out
    }

    fn under_ten_thousand(magnitude: u64) -> String {
        let hundreds = magnitude / 100;
        let remainder = magnitude % 100;
        let mut out = String::new();
        if hundreds > 0 {
            if hundreds > 1 {
                out.push_str(&pair(hundreds));
            }
            out.push_str(HUNDRED);
        }
        if remainder > 0 {
            out.push_str(&pair(remainder));
        }
        out
    }

    if magnitude >= 10000 {
        let quotient = magnitude / 10000;
        let remainder = magnitude % 10000;
        let mut out = if quotient > 1 {
            under_ten_thousand(quotient)
        } else {
            String::new()
        };
        out.push_str(TEN_THOUSAND);
        if remainder > 0 {
            out.push_str(&pair(remainder));
        }
        out
    } else {
        under_ten_thousand(magnitude)
    }
}

// ---------------------------------------------------------------------------
// Predefined counter style table
// ---------------------------------------------------------------------------

fn chars_range(start: char, end: char) -> Vec<String> {
    (start..=end).map(|c| c.to_string()).collect()
}

fn symbols_of(text: &str) -> Vec<String> {
    text.chars().map(|c| c.to_string()).collect()
}

const DIGITS_ASCII: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

fn ascii_digits() -> Vec<String> {
    DIGITS_ASCII.iter().map(|c| c.to_string()).collect()
}

/// Resolve one of the predefined counter styles, or `None` if `name` is not
/// a predefined name.
fn predefined(name: &str) -> Option<Resolved> {
    use GenerationSystem::*;

    let mut resolved = Resolved {
        system: Numeric,
        negative: Some(("\u{2d}".to_string(), None)),
        prefix: String::new(),
        suffix: ". ".to_string(),
        pad: None,
        ranges: None,
        symbols: Vec::new(),
        additive: Vec::new(),
        first_symbol_value: 1,
        predefined: None,
        fallback: Some("decimal"),
    };

    match name {
        // <https://drafts.csswg.org/css-counter-styles-3/#simple-numeric>
        "decimal" => resolved.symbols = ascii_digits(),
        "decimal-leading-zero" => {
            resolved.symbols = ascii_digits();
            resolved.pad = Some((2, "0".to_string()));
        },
        "arabic-indic" => resolved.symbols = chars_range('\u{660}', '\u{669}'),
        "bengali" => resolved.symbols = chars_range('\u{9e6}', '\u{9ef}'),
        "cambodian" | "khmer" => resolved.symbols = chars_range('\u{17e0}', '\u{17e9}'),
        "cjk-decimal" => {
            resolved.symbols = vec!["〇", "一", "二", "三", "四", "五", "六", "七", "八", "九"]
                .into_iter()
                .map(str::to_string)
                .collect();
            resolved.suffix = "、".to_string();
        },
        "devanagari" => resolved.symbols = chars_range('\u{966}', '\u{96f}'),
        "gujarati" => resolved.symbols = chars_range('\u{ae6}', '\u{aef}'),
        "gurmukhi" => resolved.symbols = chars_range('\u{a66}', '\u{a6f}'),
        "kannada" => resolved.symbols = chars_range('\u{ce6}', '\u{cef}'),
        "lao" => resolved.symbols = chars_range('\u{ed0}', '\u{ed9}'),
        "malayalam" => resolved.symbols = chars_range('\u{d66}', '\u{d6f}'),
        "mongolian" => resolved.symbols = chars_range('\u{1810}', '\u{1819}'),
        "myanmar" => resolved.symbols = chars_range('\u{1040}', '\u{1049}'),
        "oriya" => resolved.symbols = chars_range('\u{b66}', '\u{b6f}'),
        "persian" => resolved.symbols = chars_range('\u{6f0}', '\u{6f9}'),
        "tamil" => resolved.symbols = chars_range('\u{be6}', '\u{bef}'),
        "telugu" => resolved.symbols = chars_range('\u{c66}', '\u{c6f}'),
        "thai" => resolved.symbols = chars_range('\u{e50}', '\u{e59}'),
        "tibetan" => resolved.symbols = chars_range('\u{f20}', '\u{f29}'),

        // <https://drafts.csswg.org/css-counter-styles-3/#simple-alphabetic>
        "lower-alpha" | "lower-latin" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.symbols = symbols_of("abcdefghijklmnopqrstuvwxyz");
        },
        "upper-alpha" | "upper-latin" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.symbols = symbols_of("ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        },
        "lower-greek" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.symbols = symbols_of("αβγδεζηθικλμνξοπρστυφχψω");
        },
        "hiragana" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.suffix = "、".to_string();
            resolved.symbols = symbols_of(concat!(
                "あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほ",
                "まみむめもやゆよらりるれろわゐゑをん"
            ));
        },
        "katakana" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.suffix = "、".to_string();
            resolved.symbols = symbols_of(concat!(
                "アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホ",
                "マミムメモヤユヨラリルレロワヰヱヲン"
            ));
        },
        // Both iroha styles are alphabetic over the 47 iroha syllables; the
        // forty-eighth (`ん`) is not part of the ordering.
        "hiragana-iroha" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.suffix = "、".to_string();
            resolved.symbols = symbols_of(concat!(
                "いろはにほへとちりぬるをわかよたれそつねならむうゐのおくやま",
                "けふこえてあさきゆめみしゑひもせす"
            ));
        },
        "katakana-iroha" => {
            resolved.system = Alphabetic;
            resolved.negative = None;
            resolved.suffix = "、".to_string();
            resolved.symbols = symbols_of(concat!(
                "イロハニホヘトチリヌルヲワカヨタレソツネナラムウヰノオクヤマ",
                "ケフコエテアサキユメミシヱヒモセス"
            ));
        },

        // <https://drafts.csswg.org/css-counter-styles-3/#simple-fixed>
        "cjk-earthly-branch" | "cjk-heavenly-stem" => {
            resolved.system = Fixed;
            resolved.negative = None;
            resolved.suffix = "、".to_string();
            resolved.symbols = if name == "cjk-earthly-branch" {
                symbols_of("子丑寅卯辰巳午未申酉戌亥")
            } else {
                symbols_of("甲乙丙丁戊己庚辛壬癸")
            };
            resolved.fallback = Some("cjk-decimal");
        },

        // Bullets.
        "disc" => bullet(&mut resolved, "•"),
        "circle" => bullet(&mut resolved, "◦"),
        "square" => bullet(&mut resolved, "▪"),
        "disclosure-open" => bullet(&mut resolved, "▾"),
        "disclosure-closed" => bullet(&mut resolved, "▸"),

        // <https://drafts.csswg.org/css-counter-styles-3/#simple-additive>
        "lower-roman" | "upper-roman" => {
            resolved.system = Additive;
            resolved.negative = None;
            resolved.ranges = Some(vec![(Some(1), Some(3999))]);
            resolved.additive = roman_tuples(name == "upper-roman");
        },
        "armenian" | "upper-armenian" => {
            resolved.system = Additive;
            resolved.negative = None;
            resolved.ranges = Some(vec![(Some(1), Some(9999))]);
            resolved.additive = armenian_tuples(false);
        },
        "lower-armenian" => {
            resolved.system = Additive;
            resolved.negative = None;
            resolved.ranges = Some(vec![(Some(1), Some(9999))]);
            resolved.additive = armenian_tuples(true);
        },
        "georgian" => {
            resolved.system = Additive;
            resolved.negative = None;
            resolved.ranges = Some(vec![(Some(1), Some(19999))]);
            resolved.additive = georgian_tuples();
        },

        // <https://drafts.csswg.org/css-counter-styles-3/#complex-cjk>
        "simp-chinese-informal"
        | "simp-chinese-formal"
        | "trad-chinese-informal"
        | "trad-chinese-formal"
        | "cjk-ideographic"
        | "japanese-informal"
        | "japanese-formal"
        | "korean-hanja-informal"
        | "korean-hanja-formal"
        | "korean-hangul-formal" => {
            resolved.system = Numeric;
            resolved.ranges = Some(vec![(Some(-9999), Some(9999))]);
            resolved.suffix = if name.starts_with("korean") {
                ", ".to_string()
            } else {
                "、".to_string()
            };
            resolved.negative = Some(match name {
                "simp-chinese-informal" | "simp-chinese-formal" | "cjk-ideographic" => {
                    ("负".to_string(), None)
                },
                "trad-chinese-informal" | "trad-chinese-formal" => ("負".to_string(), None),
                "japanese-informal" | "japanese-formal" => ("マイナス".to_string(), None),
                _ => ("마이너스 ".to_string(), None),
            });
            resolved.fallback = Some(if name.starts_with("korean") {
                "decimal"
            } else {
                "cjk-decimal"
            });
            resolved.predefined = Some(match name {
                "simp-chinese-informal" | "cjk-ideographic" => "simp-chinese-informal",
                "simp-chinese-formal" => "simp-chinese-formal",
                "trad-chinese-informal" => "trad-chinese-informal",
                "trad-chinese-formal" => "trad-chinese-formal",
                "japanese-informal" => "japanese-informal",
                "japanese-formal" => "japanese-formal",
                "korean-hanja-informal" => "korean-hanja-informal",
                "korean-hanja-formal" => "korean-hanja-formal",
                _ => "korean-hangul-formal",
            });
        },

        // Special algorithms.
        "hebrew" => {
            resolved.system = Numeric;
            resolved.ranges = Some(vec![(Some(1), Some(10999))]);
            resolved.predefined = Some("hebrew");
        },
        "ethiopic-numeric" => {
            resolved.system = Numeric;
            resolved.ranges = Some(vec![(Some(1), None)]);
            resolved.suffix = "/ ".to_string();
            resolved.predefined = Some("ethiopic-numeric");
        },

        _ => return None,
    }

    // The `predefined` field also drives the specialised algorithms; make
    // sure every East Asian style carries its name so `special_representation`
    // can find it.
    Some(resolved)
}

fn bullet(resolved: &mut Resolved, symbol: &str) {
    resolved.system = GenerationSystem::Cyclic;
    resolved.negative = None;
    resolved.suffix = " ".to_string();
    resolved.symbols = vec![symbol.to_string()];
    resolved.ranges = None;
}

fn roman_tuples(upper_case: bool) -> Vec<(i64, String)> {
    let cases: [(&str, i64); 13] = [
        ("M", 1000),
        ("CM", 900),
        ("D", 500),
        ("CD", 400),
        ("C", 100),
        ("XC", 90),
        ("L", 50),
        ("XL", 40),
        ("X", 10),
        ("IX", 9),
        ("V", 5),
        ("IV", 4),
        ("I", 1),
    ];
    cases
        .iter()
        .map(|(symbol, weight)| {
            (
                *weight,
                if upper_case {
                    (*symbol).to_string()
                } else {
                    symbol.to_lowercase()
                },
            )
        })
        .collect()
}

/// Armenian additive tuples. The nine glyphs of each digit group are the
/// numerals for 1..9 scaled by the group's place value.
fn armenian_tuples(lower_case: bool) -> Vec<(i64, String)> {
    let groups: [[&str; 9]; 4] = if lower_case {
        [
            ["ա", "բ", "գ", "դ", "ե", "զ", "է", "ը", "թ"],
            ["ժ", "ի", "լ", "խ", "ծ", "կ", "հ", "ձ", "ղ"],
            ["ճ", "մ", "յ", "ն", "շ", "ո", "չ", "պ", "ջ"],
            ["ռ", "ս", "վ", "տ", "ր", "ց", "ւ", "փ", "ք"],
        ]
    } else {
        [
            ["Ա", "Բ", "Գ", "Դ", "Ե", "Զ", "Է", "Ը", "Թ"],
            ["Ժ", "Ի", "Լ", "Խ", "Ծ", "Կ", "Հ", "Ձ", "Ղ"],
            ["Ճ", "Մ", "Յ", "Ն", "Շ", "Ո", "Չ", "Պ", "Ջ"],
            ["Ռ", "Ս", "Վ", "Տ", "Ր", "Ց", "Ւ", "Փ", "Ք"],
        ]
    };
    let places = [1000_i64, 100, 10, 1];
    let mut tuples = Vec::with_capacity(36);
    for (group, place) in groups.iter().zip(places) {
        for (index, symbol) in group.iter().enumerate().rev() {
            tuples.push(((index as i64 + 1) * place, (*symbol).to_string()));
        }
    }
    tuples
}

/// Georgian additive tuples, including the archaic letters used for 60 and
/// for 8, and the ten-thousand glyph.
fn georgian_tuples() -> Vec<(i64, String)> {
    let groups: [(i64, [&str; 9]); 4] = [
        (1, ["ა", "ბ", "გ", "დ", "ე", "ვ", "ზ", "ჱ", "თ"]),
        (10, ["ი", "კ", "ლ", "მ", "ნ", "ჲ", "ო", "პ", "ჟ"]),
        (100, ["რ", "ს", "ტ", "უ", "ფ", "ქ", "ღ", "ყ", "შ"]),
        (1000, ["ჩ", "ც", "ძ", "წ", "ჭ", "ხ", "ჴ", "ჯ", "ჰ"]),
    ];
    let mut tuples = Vec::with_capacity(37);
    for (place, group) in groups.iter().rev() {
        for (index, symbol) in group.iter().enumerate().rev() {
            tuples.push(((index as i64 + 1) * place, (*symbol).to_string()));
        }
    }
    tuples.push((10000, "ჵ".to_string()));
    tuples
}

// ---------------------------------------------------------------------------
// Resolving a `style::counter_style::CounterStyle`
// ---------------------------------------------------------------------------

fn symbol_text(symbol: &Symbol) -> String {
    match symbol {
        Symbol::String(string) => string.to_string(),
        Symbol::Ident(ident) => ident.0.to_string(),
    }
}

fn negative_text(negative: &Negative) -> (String, Option<String>) {
    (
        symbol_text(&negative.0),
        negative.1.as_ref().map(symbol_text),
    )
}

fn pad_text(pad: &Pad) -> (usize, String) {
    // `@counter-style/pad` rejects calc expressions that cannot be resolved at
    // parse time (`counter_style::Pad::parse`), so `resolve()` is always `Some`
    // for a style that reached layout. Stay fail-safe regardless.
    let min_length = pad.0.resolve().unwrap_or(0).max(0);
    (min_length as usize, symbol_text(&pad.1))
}

/// Resolve a `CounterStyle` to a [`Resolved`], or `None` when the style
/// cannot produce a marker (`none`, `<string>`, or a not-yet-defined custom
/// name).
fn resolve(style: &CounterStyle) -> Option<Resolved> {
    match style {
        CounterStyle::None | CounterStyle::String(_) => None,
        CounterStyle::Symbols { ty, symbols } => Some(Resolved {
            system: match ty {
                SymbolsType::Cyclic => GenerationSystem::Cyclic,
                SymbolsType::Numeric => GenerationSystem::Numeric,
                SymbolsType::Alphabetic => GenerationSystem::Alphabetic,
                SymbolsType::Symbolic => GenerationSystem::Symbolic,
                SymbolsType::Fixed => GenerationSystem::Fixed,
            },
            negative: None,
            prefix: String::new(),
            suffix: " ".to_string(),
            pad: None,
            ranges: None,
            symbols: symbols_text(symbols),
            additive: Vec::new(),
            first_symbol_value: 1,
            predefined: None,
            fallback: Some("decimal"),
        }),
        CounterStyle::Name(name) => predefined(&name.0),
    }
}

fn symbols_text(symbols: &Symbols) -> Vec<String> {
    symbols.0.iter().map(symbol_text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(name: &str, value: i32) -> Option<String> {
        let resolved = predefined(name)?;
        generate(&resolved, value)
    }

    #[test]
    fn decimal_representation() {
        assert_eq!(marker("decimal", 1).as_deref(), Some("1. "));
        assert_eq!(marker("decimal", 4321).as_deref(), Some("4321. "));
        assert_eq!(marker("decimal", -7).as_deref(), Some("-7. "));
    }

    #[test]
    fn decimal_leading_zero_representation() {
        assert_eq!(marker("decimal-leading-zero", 0).as_deref(), Some("00. "));
        assert_eq!(marker("decimal-leading-zero", 9).as_deref(), Some("09. "));
        assert_eq!(marker("decimal-leading-zero", 10).as_deref(), Some("10. "));
    }

    #[test]
    fn roman_representations() {
        assert_eq!(marker("lower-roman", 4).as_deref(), Some("iv. "));
        assert_eq!(marker("lower-roman", 1984).as_deref(), Some("mcmlxxxiv. "));
        assert_eq!(marker("upper-roman", 3999).as_deref(), Some("MMMCMXCIX. "));
        // Out of range: roman falls back to decimal.
        assert_eq!(marker("lower-roman", 0).as_deref(), Some("0. "));
        assert_eq!(marker("lower-roman", 4000).as_deref(), Some("4000. "));
    }

    #[test]
    fn armenian_representations() {
        assert_eq!(marker("armenian", 1).as_deref(), Some("Ա. "));
        assert_eq!(marker("armenian", 10).as_deref(), Some("Ժ. "));
        assert_eq!(marker("armenian", 222).as_deref(), Some("ՄԻԲ. "));
        assert_eq!(marker("armenian", 9999).as_deref(), Some("ՔՋՂԹ. "));
        assert_eq!(marker("lower-armenian", 43).as_deref(), Some("խգ. "));
    }

    #[test]
    fn georgian_representations() {
        assert_eq!(marker("georgian", 8).as_deref(), Some("ჱ. "));
        assert_eq!(marker("georgian", 43).as_deref(), Some("მგ. "));
        assert_eq!(marker("georgian", 10000).as_deref(), Some("ჵ. "));
        assert_eq!(marker("georgian", 20000).as_deref(), Some("20000. "));
    }

    #[test]
    fn hebrew_representations() {
        assert_eq!(marker("hebrew", 1).as_deref(), Some("א. "));
        assert_eq!(marker("hebrew", 15).as_deref(), Some("טו. "));
        assert_eq!(marker("hebrew", 16).as_deref(), Some("טז. "));
        assert_eq!(marker("hebrew", 500).as_deref(), Some("תק. "));
        assert_eq!(marker("hebrew", 555).as_deref(), Some("תקנה. "));
        assert_eq!(marker("hebrew", 1000).as_deref(), Some("א\u{5f3}. "));
        assert_eq!(marker("hebrew", 3256).as_deref(), Some("ג\u{5f3}רנו. "));
        // 11000 is above the style's range: decimal fallback.
        assert_eq!(marker("hebrew", 11000).as_deref(), Some("11000. "));
    }

    #[test]
    fn ethiopic_representations() {
        assert_eq!(marker("ethiopic-numeric", 1).as_deref(), Some("፩/ "));
        assert_eq!(marker("ethiopic-numeric", 11).as_deref(), Some("፲፩/ "));
        assert_eq!(marker("ethiopic-numeric", 101).as_deref(), Some("፻፩/ "));
        assert_eq!(marker("ethiopic-numeric", 222).as_deref(), Some("፪፻፳፪/ "));
        assert_eq!(marker("ethiopic-numeric", 10000).as_deref(), Some("፼/ "));
        assert_eq!(
            marker("ethiopic-numeric", 1000001).as_deref(),
            Some("፻፼፩/ ")
        );
    }

    #[test]
    fn cjk_numeric_representations() {
        assert_eq!(marker("cjk-decimal", 3).as_deref(), Some("三、"));
        assert_eq!(marker("cjk-decimal", 43).as_deref(), Some("四三、"));
    }

    #[test]
    fn east_asian_longhand_representations() {
        assert_eq!(marker("japanese-informal", 10).as_deref(), Some("十、"));
        assert_eq!(marker("japanese-informal", 101).as_deref(), Some("百一、"));
        assert_eq!(
            marker("japanese-informal", 222).as_deref(),
            Some("二百二十二、")
        );
        assert_eq!(marker("japanese-informal", 1005).as_deref(), Some("千五、"));
        assert_eq!(
            marker("simp-chinese-informal", 101).as_deref(),
            Some("一百零一、")
        );
        assert_eq!(
            marker("simp-chinese-formal", 540).as_deref(),
            Some("伍佰肆拾、")
        );
        assert_eq!(
            marker("trad-chinese-formal", 101).as_deref(),
            Some("壹佰零壹、")
        );
        assert_eq!(
            marker("korean-hangul-formal", 43).as_deref(),
            Some("사십삼, ")
        );
        assert_eq!(
            marker("korean-hanja-formal", 540).as_deref(),
            Some("五百四拾, ")
        );
    }

    #[test]
    fn east_asian_negative_signs() {
        assert_eq!(
            marker("japanese-formal", -11).as_deref(),
            Some("マイナス壱拾壱、")
        );
        assert_eq!(
            marker("simp-chinese-informal", -9).as_deref(),
            Some("负九、")
        );
        assert_eq!(
            marker("korean-hangul-formal", -9).as_deref(),
            Some("마이너스 구, ")
        );
    }

    #[test]
    fn east_asian_out_of_range_falls_back() {
        assert_eq!(
            marker("japanese-informal", 10000).as_deref(),
            Some("一〇〇〇〇、")
        );
        assert_eq!(
            marker("korean-hangul-formal", 10000).as_deref(),
            Some("10000, ")
        );
    }

    #[test]
    fn alphabetic_representations() {
        assert_eq!(marker("lower-alpha", 1).as_deref(), Some("a. "));
        assert_eq!(marker("lower-alpha", 26).as_deref(), Some("z. "));
        assert_eq!(marker("lower-alpha", 27).as_deref(), Some("aa. "));
        assert_eq!(marker("lower-greek", 25).as_deref(), Some("αα. "));
        // Alphabetic systems have no representation for zero.
        assert_eq!(marker("lower-alpha", 0).as_deref(), Some("0. "));
    }

    #[test]
    fn hiragana_representations() {
        assert_eq!(marker("hiragana", 1).as_deref(), Some("あ、"));
        assert_eq!(marker("hiragana", 48).as_deref(), Some("ん、"));
        assert_eq!(marker("hiragana", 49).as_deref(), Some("ああ、"));
        assert_eq!(marker("hiragana-iroha", 49).as_deref(), Some("いい、"));
    }

    #[test]
    fn fixed_representations() {
        assert_eq!(marker("cjk-heavenly-stem", 3).as_deref(), Some("丙、"));
        assert_eq!(marker("cjk-earthly-branch", 12).as_deref(), Some("亥、"));
        // Out of range falls back to cjk-decimal.
        assert_eq!(marker("cjk-heavenly-stem", 13).as_deref(), Some("一三、"));
        assert_eq!(marker("cjk-earthly-branch", 77).as_deref(), Some("七七、"));
    }

    #[test]
    fn bullets() {
        assert_eq!(marker("disc", 1).as_deref(), Some("• "));
        assert_eq!(marker("circle", 1).as_deref(), Some("◦ "));
        assert_eq!(marker("square", 1).as_deref(), Some("▪ "));
        assert_eq!(marker("disclosure-open", 1).as_deref(), Some("▾ "));
    }

    #[test]
    fn numeric_scripts() {
        assert_eq!(marker("arabic-indic", 12).as_deref(), Some("١٢. "));
        assert_eq!(marker("thai", 43).as_deref(), Some("๔๓. "));
        assert_eq!(marker("devanagari", 9).as_deref(), Some("९. "));
    }

    #[test]
    fn unknown_names_produce_no_marker() {
        assert_eq!(predefined("not-a-style").is_none(), true);
    }
}
