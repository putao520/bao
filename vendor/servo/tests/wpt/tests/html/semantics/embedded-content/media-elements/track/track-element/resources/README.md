# BAO patch (fork-maintained, 2026-09-27): WebVTT parser test fixtures.
#
# The upstream servo checkout keeps these under tests/wpt/...; bao does not
# vendor the WPT suite, so only the fixtures the `bao-servo-webvtt` crate
# tests actually `include_str!` are vendored here (REQ-BRW-047 absorption
# wave), byte-identical to upstream 7ca99fe3f.
