//! Strata's curated color schemes.
//!
//! Each scheme is a palette transcribed from its project's published,
//! permissively licensed color specification (MIT or Apache-2.0; see each
//! `Credit`), mapped onto one shared set of scope rules by [`build`]. Sharing
//! the rules means every scheme colors every language the same way: comments,
//! strings, keywords, functions, types, constants, numbers, operators,
//! punctuation, tags, attributes, Markdown and diffs all get a color, and every
//! scheme defines the editor colors Strata paints (caret, selection, current
//! line, gutter, find matches, indent guides, bracket match, whitespace).
//!
//! Legibility rule: text must reach WCAG AA (4.5:1) against the background and
//! every syntax color at least 3:1 (high-contrast schemes: 7:1 and 4.5:1). A
//! color in a palette that falls short (some schemes ship very faint comments)
//! is nudged toward the foreground's extreme until it passes, keeping its hue.

use super::{Scheme, rgb_color};
use egui::Color32;
use std::str::FromStr;
use syntect::highlighting::{FontStyle, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dark,
    Light,
    HighContrastDark,
    HighContrastLight,
}

impl Kind {
    pub fn is_light(self) -> bool {
        matches!(self, Kind::Light | Kind::HighContrastLight)
    }

    pub fn is_high_contrast(self) -> bool {
        matches!(self, Kind::HighContrastDark | Kind::HighContrastLight)
    }
}

/// Where a palette comes from, for Help ▸ Licenses.
pub struct Credit {
    pub project: &'static str,
    pub author: &'static str,
    pub license: &'static str,
    pub url: &'static str,
}

/// Italic flags.
pub const IC: u8 = 1; // comments
pub const IK: u8 = 2; // keywords and storage
pub const IT: u8 = 4; // types
pub const IP: u8 = 8; // parameters
pub const IA: u8 = 16; // attributes

/// One scheme's palette. Colors are 0xRRGGBB.
pub struct Spec {
    pub name: &'static str,
    pub kind: Kind,
    pub credit: &'static Credit,
    // Editor UI
    pub bg: u32,
    pub fg: u32,
    pub caret: u32,
    pub selection: u32,
    /// Current line.
    pub line: u32,
    /// Line numbers.
    pub gutter: u32,
    /// Find matches.
    pub find: u32,
    /// Indent guides and rulers.
    pub guide: u32,
    /// The bracket pair around the caret.
    pub bracket: u32,
    /// Scrollbar, active tab and other highlights in the chrome.
    pub accent: u32,
    pub whitespace: u32,
    // Syntax
    pub comment: u32,
    pub string: u32,
    /// Escapes, regular expressions, format placeholders.
    pub escape: u32,
    pub keyword: u32,
    pub operator: u32,
    pub function: u32,
    pub ty: u32,
    pub constant: u32,
    pub number: u32,
    pub variable: u32,
    pub parameter: u32,
    /// Fields, object keys, JSON/YAML keys, CSS properties.
    pub property: u32,
    pub punctuation: u32,
    pub tag: u32,
    pub attribute: u32,
    /// Macros, decorators, annotations, preprocessor.
    pub macros: u32,
    pub heading: u32,
    pub link: u32,
    /// Markdown code spans and blocks.
    pub code: u32,
    pub inserted: u32,
    pub deleted: u32,
    pub changed: u32,
    pub invalid: u32,
    /// Terminal hues: red, green, yellow, blue, magenta, cyan.
    pub ansi: [u32; 6],
    pub italic: u8,
}

/// Blend two 0xRRGGBB colors at compile time (`pct` of `b`).
const fn mix(a: u32, b: u32, pct: u32) -> u32 {
    const fn m(a: u32, b: u32, pct: u32, shift: u32) -> u32 {
        let (x, y) = ((a >> shift) & 0xff, (b >> shift) & 0xff);
        ((x * (100 - pct) + y * pct + 50) / 100) << shift
    }
    m(a, b, pct, 16) | m(a, b, pct, 8) | m(a, b, pct, 0)
}

const ATOM: Credit = Credit { project: "Atom One Dark / One Light syntax", author: "GitHub Inc.", license: "MIT", url: "github.com/atom/atom" };
const DRACULA: Credit = Credit { project: "Dracula", author: "Zeno Rocha and contributors", license: "MIT", url: "draculatheme.com" };
const NORD: Credit = Credit { project: "Nord", author: "Arctic Ice Studio and Sven Greb", license: "MIT", url: "nordtheme.com" };
const SOLARIZED: Credit = Credit { project: "Solarized", author: "Ethan Schoonover", license: "MIT", url: "ethanschoonover.com/solarized" };
const GRUVBOX: Credit = Credit { project: "gruvbox", author: "Pavel Pertsev (morhetz)", license: "MIT", url: "github.com/morhetz/gruvbox" };
const TOKYO: Credit = Credit { project: "Tokyo Night", author: "enkia (and folke's tokyonight.nvim)", license: "MIT", url: "github.com/enkia/tokyo-night-vscode-theme" };
const CATPPUCCIN: Credit = Credit { project: "Catppuccin", author: "Catppuccin Org", license: "MIT", url: "catppuccin.com" };
const GITHUB: Credit = Credit { project: "GitHub VS Code theme (Primer colors)", author: "GitHub Inc.", license: "MIT", url: "github.com/primer/github-vscode-theme" };
const MONOKAI: Credit = Credit { project: "Monokai Extended", author: "Jon Schlinkert (after Wimer Hazenberg's Monokai)", license: "MIT", url: "github.com/jonschlinkert/sublime-monokai-extended" };
const AYU: Credit = Credit { project: "ayu", author: "Ike Ku and contributors", license: "MIT", url: "github.com/ayu-theme/ayu-colors" };
const NIGHT_OWL: Credit = Credit { project: "Night Owl", author: "Sarah Drasner", license: "MIT", url: "github.com/sdras/night-owl-vscode-theme" };
const MATERIAL: Credit = Credit { project: "Material Theme (Palenight)", author: "Mattia Astorino", license: "Apache-2.0", url: "github.com/material-theme/vsc-material-theme" };
const ROSE_PINE: Credit = Credit { project: "Rosé Pine", author: "Rosé Pine contributors", license: "MIT", url: "rosepinetheme.com" };
const EVERFOREST: Credit = Credit { project: "Everforest", author: "sainnhe", license: "MIT", url: "github.com/sainnhe/everforest" };
const KANAGAWA: Credit = Credit { project: "Kanagawa", author: "Tommaso Laurenzi (rebelot)", license: "MIT", url: "github.com/rebelot/kanagawa.nvim" };
const STRATA: Credit = Credit { project: "Strata High Contrast", author: "Strata", license: "MIT", url: "" };

// ---- palettes ------------------------------------------------------------------

const ONE_DARK: Spec = Spec {
    name: "One Dark", kind: Kind::Dark, credit: &ATOM,
    bg: 0x282c34, fg: 0xabb2bf, caret: 0x528bff, selection: 0x3e4451, line: 0x2c313c, gutter: 0x636d83,
    find: 0xd19a66, guide: 0x3b4048, bracket: 0x61afef, accent: 0x61afef, whitespace: 0x4b5263,
    comment: 0x7f848e, string: 0x98c379, escape: 0x56b6c2, keyword: 0xc678dd, operator: 0x56b6c2,
    function: 0x61afef, ty: 0xe5c07b, constant: 0xd19a66, number: 0xd19a66, variable: 0xe06c75,
    parameter: 0xabb2bf, property: 0xe06c75, punctuation: 0xabb2bf, tag: 0xe06c75, attribute: 0xd19a66,
    macros: 0x56b6c2, heading: 0xe06c75, link: 0x61afef, code: 0x98c379,
    inserted: 0x98c379, deleted: 0xe06c75, changed: 0xd19a66, invalid: 0xf44747,
    ansi: [0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2], italic: IC,
};

const ONE_LIGHT: Spec = Spec {
    name: "One Light", kind: Kind::Light, credit: &ATOM,
    bg: 0xfafafa, fg: 0x383a42, caret: 0x526fff, selection: 0xd8dbe2, line: 0xf0f0f1, gutter: 0x9d9d9f,
    find: 0xc18401, guide: 0xdbdbdc, bracket: 0x4078f2, accent: 0x4078f2, whitespace: 0xc5c5c8,
    comment: 0xa0a1a7, string: 0x50a14f, escape: 0x0184bc, keyword: 0xa626a4, operator: 0x0184bc,
    function: 0x4078f2, ty: 0xc18401, constant: 0x986801, number: 0x986801, variable: 0xe45649,
    parameter: 0x383a42, property: 0xe45649, punctuation: 0x383a42, tag: 0xe45649, attribute: 0x986801,
    macros: 0x0184bc, heading: 0xe45649, link: 0x4078f2, code: 0x50a14f,
    inserted: 0x50a14f, deleted: 0xe45649, changed: 0x986801, invalid: 0xca1243,
    ansi: [0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc], italic: IC,
};

const DRACULA_SPEC: Spec = Spec {
    name: "Dracula", kind: Kind::Dark, credit: &DRACULA,
    bg: 0x282a36, fg: 0xf8f8f2, caret: 0xf8f8f0, selection: 0x44475a, line: 0x313342, gutter: 0x6272a4,
    find: 0xffb86c, guide: 0x3b3e4f, bracket: 0xff79c6, accent: 0xbd93f9, whitespace: 0x4d5066,
    comment: 0x6272a4, string: 0xf1fa8c, escape: 0xff79c6, keyword: 0xff79c6, operator: 0xff79c6,
    function: 0x50fa7b, ty: 0x8be9fd, constant: 0xbd93f9, number: 0xbd93f9, variable: 0xf8f8f2,
    parameter: 0xffb86c, property: 0x8be9fd, punctuation: 0xf8f8f2, tag: 0xff79c6, attribute: 0x50fa7b,
    macros: 0x50fa7b, heading: 0xbd93f9, link: 0x8be9fd, code: 0x50fa7b,
    inserted: 0x50fa7b, deleted: 0xff5555, changed: 0xffb86c, invalid: 0xff5555,
    ansi: [0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd], italic: IT | IP | IA,
};

const NORD_SPEC: Spec = Spec {
    name: "Nord", kind: Kind::Dark, credit: &NORD,
    bg: 0x2e3440, fg: 0xd8dee9, caret: 0xd8dee9, selection: 0x434c5e, line: 0x3b4252, gutter: 0x616e88,
    find: 0x88c0d0, guide: 0x3b4252, bracket: 0x88c0d0, accent: 0x88c0d0, whitespace: 0x4c566a,
    comment: 0x616e88, string: 0xa3be8c, escape: 0xebcb8b, keyword: 0x81a1c1, operator: 0x81a1c1,
    function: 0x88c0d0, ty: 0x8fbcbb, constant: 0x81a1c1, number: 0xb48ead, variable: 0xd8dee9,
    parameter: 0xd8dee9, property: 0x8fbcbb, punctuation: 0xeceff4, tag: 0x81a1c1, attribute: 0x8fbcbb,
    macros: 0x5e81ac, heading: 0x88c0d0, link: 0x88c0d0, code: 0xa3be8c,
    inserted: 0xa3be8c, deleted: 0xbf616a, changed: 0xebcb8b, invalid: 0xbf616a,
    ansi: [0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0], italic: IC,
};

const SOLARIZED_DARK: Spec = Spec {
    name: "Solarized Dark", kind: Kind::Dark, credit: &SOLARIZED,
    bg: 0x002b36, fg: 0x93a1a1, caret: 0xd30102, selection: 0x274642, line: 0x073642, gutter: 0x586e75,
    find: 0xb58900, guide: 0x0a3d48, bracket: 0xcb4b16, accent: 0x268bd2, whitespace: 0x1d4651,
    comment: 0x586e75, string: 0x2aa198, escape: 0xcb4b16, keyword: 0x859900, operator: 0x859900,
    function: 0x268bd2, ty: 0xb58900, constant: 0xcb4b16, number: 0xd33682, variable: 0x93a1a1,
    parameter: 0x93a1a1, property: 0x268bd2, punctuation: 0x839496, tag: 0x268bd2, attribute: 0xb58900,
    macros: 0xcb4b16, heading: 0x268bd2, link: 0x6c71c4, code: 0x2aa198,
    inserted: 0x859900, deleted: 0xdc322f, changed: 0xb58900, invalid: 0xdc322f,
    ansi: [0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198], italic: IC,
};

const SOLARIZED_LIGHT: Spec = Spec {
    name: "Solarized Light", kind: Kind::Light, credit: &SOLARIZED,
    bg: 0xfdf6e3, fg: 0x586e75, caret: 0x657b83, selection: 0xe4ddc8, line: 0xeee8d5, gutter: 0x93a1a1,
    find: 0xb58900, guide: 0xeae3cf, bracket: 0xcb4b16, accent: 0x268bd2, whitespace: 0xd6cfba,
    comment: 0x93a1a1, string: 0x2aa198, escape: 0xcb4b16, keyword: 0x859900, operator: 0x859900,
    function: 0x268bd2, ty: 0xb58900, constant: 0xcb4b16, number: 0xd33682, variable: 0x586e75,
    parameter: 0x586e75, property: 0x268bd2, punctuation: 0x657b83, tag: 0x268bd2, attribute: 0xb58900,
    macros: 0xcb4b16, heading: 0x268bd2, link: 0x6c71c4, code: 0x2aa198,
    inserted: 0x859900, deleted: 0xdc322f, changed: 0xb58900, invalid: 0xdc322f,
    ansi: [0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198], italic: IC,
};

const GRUVBOX_DARK: Spec = Spec {
    name: "Gruvbox Dark", kind: Kind::Dark, credit: &GRUVBOX,
    bg: 0x282828, fg: 0xebdbb2, caret: 0xebdbb2, selection: 0x504945, line: 0x32302f, gutter: 0x7c6f64,
    find: 0xfabd2f, guide: 0x3c3836, bracket: 0xfe8019, accent: 0xfe8019, whitespace: 0x504945,
    comment: 0x928374, string: 0xb8bb26, escape: 0xfe8019, keyword: 0xfb4934, operator: 0xfe8019,
    function: 0x8ec07c, ty: 0xfabd2f, constant: 0xd3869b, number: 0xd3869b, variable: 0xebdbb2,
    parameter: 0x83a598, property: 0x83a598, punctuation: 0xa89984, tag: 0x8ec07c, attribute: 0xfabd2f,
    macros: 0x8ec07c, heading: 0xfabd2f, link: 0x83a598, code: 0xb8bb26,
    inserted: 0xb8bb26, deleted: 0xfb4934, changed: 0x83a598, invalid: 0xfb4934,
    ansi: [0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c], italic: IC,
};

const GRUVBOX_LIGHT: Spec = Spec {
    name: "Gruvbox Light", kind: Kind::Light, credit: &GRUVBOX,
    bg: 0xfbf1c7, fg: 0x3c3836, caret: 0x3c3836, selection: 0xd5c4a1, line: 0xf2e5bc, gutter: 0x928374,
    find: 0xb57614, guide: 0xebdbb2, bracket: 0xaf3a03, accent: 0xaf3a03, whitespace: 0xd5c4a1,
    comment: 0x928374, string: 0x79740e, escape: 0xaf3a03, keyword: 0x9d0006, operator: 0xaf3a03,
    function: 0x427b58, ty: 0xb57614, constant: 0x8f3f71, number: 0x8f3f71, variable: 0x3c3836,
    parameter: 0x076678, property: 0x076678, punctuation: 0x665c54, tag: 0x427b58, attribute: 0xb57614,
    macros: 0x427b58, heading: 0xb57614, link: 0x076678, code: 0x79740e,
    inserted: 0x79740e, deleted: 0x9d0006, changed: 0x076678, invalid: 0x9d0006,
    ansi: [0x9d0006, 0x79740e, 0xb57614, 0x076678, 0x8f3f71, 0x427b58], italic: IC,
};

const TOKYO_NIGHT: Spec = Spec {
    name: "Tokyo Night", kind: Kind::Dark, credit: &TOKYO,
    bg: 0x1a1b26, fg: 0xc0caf5, caret: 0xc0caf5, selection: 0x2e3c64, line: 0x1f2335, gutter: 0x545c7e,
    find: 0xff9e64, guide: 0x292e42, bracket: 0x7aa2f7, accent: 0x7aa2f7, whitespace: 0x3b4261,
    comment: 0x565f89, string: 0x9ece6a, escape: 0x89ddff, keyword: 0xbb9af7, operator: 0x89ddff,
    function: 0x7aa2f7, ty: 0x2ac3de, constant: 0xff9e64, number: 0xff9e64, variable: 0xc0caf5,
    parameter: 0xe0af68, property: 0x73daca, punctuation: 0x9abdf5, tag: 0xf7768e, attribute: 0xbb9af7,
    macros: 0x7dcfff, heading: 0x7aa2f7, link: 0x73daca, code: 0x9ece6a,
    inserted: 0x9ece6a, deleted: 0xf7768e, changed: 0xe0af68, invalid: 0xff5370,
    ansi: [0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff], italic: IC,
};

const TOKYO_STORM: Spec = Spec {
    name: "Tokyo Night Storm", kind: Kind::Dark, credit: &TOKYO,
    bg: 0x24283b, line: 0x292e42, selection: 0x364a82, guide: 0x2f344d, gutter: 0x5a6182,
    ..TOKYO_NIGHT
};

const TOKYO_DAY: Spec = Spec {
    name: "Tokyo Night Day", kind: Kind::Light, credit: &TOKYO,
    bg: 0xe1e2e7, fg: 0x3760bf, caret: 0x3760bf, selection: 0xb7c1e3, line: 0xd5d6db, gutter: 0x8990b3,
    find: 0xb15c00, guide: 0xc4c8da, bracket: 0x2e7de9, accent: 0x2e7de9, whitespace: 0xa8aecb,
    comment: 0x848cb5, string: 0x587539, escape: 0x006a83, keyword: 0x9854f1, operator: 0x006a83,
    function: 0x2e7de9, ty: 0x007197, constant: 0xb15c00, number: 0xb15c00, variable: 0x3760bf,
    parameter: 0x8c6c3e, property: 0x387068, punctuation: 0x006a83, tag: 0xf52a65, attribute: 0x9854f1,
    macros: 0x007197, heading: 0x2e7de9, link: 0x387068, code: 0x587539,
    inserted: 0x587539, deleted: 0xf52a65, changed: 0x8c6c3e, invalid: 0xc64343,
    ansi: [0xf52a65, 0x587539, 0x8c6c3e, 0x2e7de9, 0x9854f1, 0x007197], italic: IC,
};

/// Catppuccin flavors share one mapping (the project's style guide).
#[allow(clippy::too_many_arguments)]
const fn catppuccin(
    name: &'static str, kind: Kind,
    base: u32, surface0: u32, surface1: u32, overlay0: u32, overlay2: u32, text: u32,
    rosewater: u32, pink: u32, mauve: u32, red: u32, maroon: u32, peach: u32, yellow: u32,
    green: u32, teal: u32, sky: u32, blue: u32, lavender: u32,
) -> Spec {
    Spec {
        name, kind, credit: &CATPPUCCIN,
        bg: base, fg: text, caret: rosewater, selection: mix(base, overlay2, 30), line: mix(base, surface0, 55), gutter: overlay0,
        find: peach, guide: surface0, bracket: overlay2, accent: mauve, whitespace: surface1,
        comment: overlay2, string: green, escape: pink, keyword: mauve, operator: sky,
        function: blue, ty: yellow, constant: peach, number: peach, variable: text,
        parameter: maroon, property: lavender, punctuation: overlay2, tag: blue, attribute: yellow,
        macros: teal, heading: red, link: lavender, code: teal,
        inserted: green, deleted: red, changed: yellow, invalid: red,
        ansi: [red, green, yellow, blue, pink, teal], italic: IC | IP,
    }
}

const MOCHA: Spec = catppuccin(
    "Catppuccin Mocha", Kind::Dark, 0x1e1e2e, 0x313244, 0x45475a, 0x6c7086, 0x9399b2, 0xcdd6f4,
    0xf5e0dc, 0xf5c2e7, 0xcba6f7, 0xf38ba8, 0xeba0ac, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x89dceb, 0x89b4fa, 0xb4befe,
);
const MACCHIATO: Spec = catppuccin(
    "Catppuccin Macchiato", Kind::Dark, 0x24273a, 0x363a4f, 0x494d64, 0x6e738d, 0x939ab7, 0xcad3f5,
    0xf4dbd6, 0xf5bde6, 0xc6a0f6, 0xed8796, 0xee99a0, 0xf5a97f, 0xeed49f, 0xa6da95, 0x8bd5ca, 0x91d7e3, 0x8aadf4, 0xb7bdf8,
);
const FRAPPE: Spec = catppuccin(
    "Catppuccin Frappé", Kind::Dark, 0x303446, 0x414559, 0x51576d, 0x737994, 0x949cbb, 0xc6d0f5,
    0xf2d5cf, 0xf4b8e4, 0xca9ee6, 0xe78284, 0xea999c, 0xef9f76, 0xe5c890, 0xa6d189, 0x81c8be, 0x99d1db, 0x8caaee, 0xbabbf1,
);
const LATTE: Spec = catppuccin(
    "Catppuccin Latte", Kind::Light, 0xeff1f5, 0xccd0da, 0xbcc0cc, 0x9ca0b0, 0x7c7f93, 0x4c4f69,
    0xdc8a78, 0xea76cb, 0x8839ef, 0xd20f39, 0xe64553, 0xfe640b, 0xdf8e1d, 0x40a02b, 0x179299, 0x04a5e5, 0x1e66f5, 0x7287fd,
);

const GITHUB_DARK: Spec = Spec {
    name: "GitHub Dark", kind: Kind::Dark, credit: &GITHUB,
    bg: 0x0d1117, fg: 0xe6edf3, caret: 0x2f81f7, selection: 0x1c3a5e, line: 0x161b22, gutter: 0x6e7681,
    find: 0xd29922, guide: 0x21262d, bracket: 0x79c0ff, accent: 0x2f81f7, whitespace: 0x484f58,
    comment: 0x8b949e, string: 0xa5d6ff, escape: 0x7ee787, keyword: 0xff7b72, operator: 0xff7b72,
    function: 0xd2a8ff, ty: 0xffa657, constant: 0x79c0ff, number: 0x79c0ff, variable: 0xe6edf3,
    parameter: 0xffa657, property: 0x79c0ff, punctuation: 0xc9d1d9, tag: 0x7ee787, attribute: 0x79c0ff,
    macros: 0xd2a8ff, heading: 0x79c0ff, link: 0xa5d6ff, code: 0x79c0ff,
    inserted: 0x7ee787, deleted: 0xffa198, changed: 0xffa657, invalid: 0xf85149,
    ansi: [0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf], italic: 0,
};

const GITHUB_LIGHT: Spec = Spec {
    name: "GitHub Light", kind: Kind::Light, credit: &GITHUB,
    bg: 0xffffff, fg: 0x1f2328, caret: 0x0969da, selection: 0xc8e1ff, line: 0xf6f8fa, gutter: 0x8c959f,
    find: 0xbf8700, guide: 0xe6eaef, bracket: 0x0969da, accent: 0x0969da, whitespace: 0xd0d7de,
    comment: 0x6e7781, string: 0x0a3069, escape: 0x116329, keyword: 0xcf222e, operator: 0xcf222e,
    function: 0x8250df, ty: 0x953800, constant: 0x0550ae, number: 0x0550ae, variable: 0x1f2328,
    parameter: 0x953800, property: 0x0550ae, punctuation: 0x1f2328, tag: 0x116329, attribute: 0x0550ae,
    macros: 0x8250df, heading: 0x0550ae, link: 0x0a3069, code: 0x0550ae,
    inserted: 0x116329, deleted: 0x82071e, changed: 0x953800, invalid: 0x82071e,
    ansi: [0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83], italic: 0,
};

const MONOKAI_SPEC: Spec = Spec {
    name: "Monokai Extended", kind: Kind::Dark, credit: &MONOKAI,
    bg: 0x272822, fg: 0xf8f8f2, caret: 0xf8f8f0, selection: 0x49483e, line: 0x3e3d32, gutter: 0x90908a,
    find: 0xe6db74, guide: 0x3b3a32, bracket: 0xfd971f, accent: 0xa6e22e, whitespace: 0x464741,
    comment: 0x75715e, string: 0xe6db74, escape: 0xae81ff, keyword: 0xf92672, operator: 0xf92672,
    function: 0xa6e22e, ty: 0x66d9ef, constant: 0xae81ff, number: 0xae81ff, variable: 0xf8f8f2,
    parameter: 0xfd971f, property: 0xf8f8f2, punctuation: 0xf8f8f2, tag: 0xf92672, attribute: 0xa6e22e,
    macros: 0xa6e22e, heading: 0xfd971f, link: 0x66d9ef, code: 0xae81ff,
    inserted: 0xa6e22e, deleted: 0xf92672, changed: 0xe6db74, invalid: 0xf92672,
    ansi: [0xf92672, 0xa6e22e, 0xe6db74, 0x66d9ef, 0xae81ff, 0xa1efe4], italic: IT | IP,
};

const AYU_DARK: Spec = Spec {
    name: "Ayu Dark", kind: Kind::Dark, credit: &AYU,
    bg: 0x0b0e14, fg: 0xbfbdb6, caret: 0xe6b450, selection: 0x1b3a5b, line: 0x131721, gutter: 0x6c7380,
    find: 0xe6b450, guide: 0x1b1f29, bracket: 0xe6b450, accent: 0xe6b450, whitespace: 0x2d3640,
    comment: 0x5a6378, string: 0xaad94c, escape: 0x95e6cb, keyword: 0xff8f40, operator: 0xf29668,
    function: 0xffb454, ty: 0x59c2ff, constant: 0xd2a6ff, number: 0xd2a6ff, variable: 0xbfbdb6,
    parameter: 0xd2a6ff, property: 0x39bae6, punctuation: 0xbfbdb6, tag: 0x39bae6, attribute: 0xffb454,
    macros: 0xe6b673, heading: 0xf07178, link: 0x39bae6, code: 0x95e6cb,
    inserted: 0x7fd962, deleted: 0xf26d78, changed: 0x73b8ff, invalid: 0xd95757,
    ansi: [0xf07178, 0xaad94c, 0xffb454, 0x59c2ff, 0xd2a6ff, 0x95e6cb], italic: IC,
};

const AYU_MIRAGE: Spec = Spec {
    name: "Ayu Mirage", kind: Kind::Dark, credit: &AYU,
    bg: 0x1f2430, fg: 0xcccac2, caret: 0xffcc66, selection: 0x33415e, line: 0x1a1f29, gutter: 0x707a8c,
    find: 0xffcc66, guide: 0x2d3340, bracket: 0xffcc66, accent: 0xffcc66, whitespace: 0x3d4555,
    comment: 0x6e7c8f, string: 0xd5ff80, escape: 0x95e6cb, keyword: 0xffad66, operator: 0xf29e74,
    function: 0xffd173, ty: 0x73d0ff, constant: 0xdfbfff, number: 0xdfbfff, variable: 0xcccac2,
    parameter: 0xdfbfff, property: 0x5ccfe6, punctuation: 0xcccac2, tag: 0x5ccfe6, attribute: 0xffd173,
    macros: 0xffdfb3, heading: 0xf28779, link: 0x5ccfe6, code: 0x95e6cb,
    inserted: 0x87d96c, deleted: 0xf27983, changed: 0x80bfff, invalid: 0xff6666,
    ansi: [0xf28779, 0xd5ff80, 0xffd173, 0x73d0ff, 0xdfbfff, 0x95e6cb], italic: IC,
};

const AYU_LIGHT: Spec = Spec {
    name: "Ayu Light", kind: Kind::Light, credit: &AYU,
    bg: 0xfcfcfc, fg: 0x5c6166, caret: 0xffaa33, selection: 0xd8e6f4, line: 0xf2f3f4, gutter: 0x8a9199,
    find: 0xffaa33, guide: 0xe8eaed, bracket: 0xffaa33, accent: 0xffaa33, whitespace: 0xd2d4d8,
    comment: 0x8a9199, string: 0x86b300, escape: 0x4cbf99, keyword: 0xfa8d3e, operator: 0xed9366,
    function: 0xf2ae49, ty: 0x399ee6, constant: 0xa37acc, number: 0xa37acc, variable: 0x5c6166,
    parameter: 0xa37acc, property: 0x55b4d4, punctuation: 0x5c6166, tag: 0x55b4d4, attribute: 0xf2ae49,
    macros: 0xe6ba7e, heading: 0xf07171, link: 0x55b4d4, code: 0x4cbf99,
    inserted: 0x6cbf43, deleted: 0xff7383, changed: 0x478acc, invalid: 0xe65050,
    ansi: [0xf07171, 0x86b300, 0xf2ae49, 0x399ee6, 0xa37acc, 0x4cbf99], italic: IC,
};

const NIGHT_OWL_SPEC: Spec = Spec {
    name: "Night Owl", kind: Kind::Dark, credit: &NIGHT_OWL,
    bg: 0x011627, fg: 0xd6deeb, caret: 0x80a4c2, selection: 0x1d3b53, line: 0x0b2238, gutter: 0x4b6479,
    find: 0x82aaff, guide: 0x122d42, bracket: 0x7fdbca, accent: 0x7e57c2, whitespace: 0x2e4a60,
    comment: 0x637777, string: 0xecc48d, escape: 0xf78c6c, keyword: 0xc792ea, operator: 0xc792ea,
    function: 0x82aaff, ty: 0xffcb8b, constant: 0xff5874, number: 0xf78c6c, variable: 0xd6deeb,
    parameter: 0xd7dbe0, property: 0xbaebe2, punctuation: 0xd9f5dd, tag: 0xcaece6, attribute: 0xaddb67,
    macros: 0x82aaff, heading: 0x82b1ff, link: 0xff869a, code: 0x80cbc4,
    inserted: 0xaddb67, deleted: 0xef5350, changed: 0xe2b93d, invalid: 0xef5350,
    ansi: [0xef5350, 0x22da6e, 0xc5e478, 0x82aaff, 0xc792ea, 0x21c7a8], italic: IC | IK | IA,
};

const LIGHT_OWL: Spec = Spec {
    name: "Night Owl Light", kind: Kind::Light, credit: &NIGHT_OWL,
    bg: 0xfbfbfb, fg: 0x403f53, caret: 0x90a7b2, selection: 0xe0e0e0, line: 0xf0f0f0, gutter: 0x90a7b2,
    find: 0x4876d6, guide: 0xe6e6e6, bracket: 0x0c969b, accent: 0x4876d6, whitespace: 0xd0d0d0,
    comment: 0x989fb1, string: 0xc96765, escape: 0xaa0982, keyword: 0x994cc3, operator: 0x0c969b,
    function: 0x4876d6, ty: 0x111111, constant: 0xbc5454, number: 0xaa0982, variable: 0x403f53,
    parameter: 0x403f53, property: 0x0c969b, punctuation: 0x994cc3, tag: 0x994cc3, attribute: 0x4876d6,
    macros: 0x4876d6, heading: 0x4876d6, link: 0x0c969b, code: 0x0c969b,
    inserted: 0x08916a, deleted: 0xde3d3b, changed: 0xdaaa01, invalid: 0xde3d3b,
    ansi: [0xde3d3b, 0x08916a, 0xdaaa01, 0x288ed7, 0xd6438a, 0x2aa298], italic: IC | IK | IA,
};

const PALENIGHT: Spec = Spec {
    name: "Material Palenight", kind: Kind::Dark, credit: &MATERIAL,
    bg: 0x292d3e, fg: 0xa6accd, caret: 0xffcc00, selection: 0x3c435e, line: 0x31354a, gutter: 0x676e95,
    find: 0xffcc00, guide: 0x3a3f58, bracket: 0x89ddff, accent: 0x80cbc4, whitespace: 0x4e5579,
    comment: 0x676e95, string: 0xc3e88d, escape: 0x89ddff, keyword: 0xc792ea, operator: 0x89ddff,
    function: 0x82aaff, ty: 0xffcb6b, constant: 0xf78c6c, number: 0xf78c6c, variable: 0xa6accd,
    parameter: 0xf78c6c, property: 0x80cbc4, punctuation: 0x89ddff, tag: 0xf07178, attribute: 0xc792ea,
    macros: 0x82aaff, heading: 0x82aaff, link: 0x89ddff, code: 0xc3e88d,
    inserted: 0xc3e88d, deleted: 0xf07178, changed: 0xffcb6b, invalid: 0xff5370,
    ansi: [0xf07178, 0xc3e88d, 0xffcb6b, 0x82aaff, 0xc792ea, 0x89ddff], italic: IC,
};

/// Rosé Pine variants share the project's role mapping.
#[allow(clippy::too_many_arguments)]
const fn rose_pine(
    name: &'static str, kind: Kind,
    base: u32, overlay: u32, muted: u32, subtle: u32, text: u32, love: u32, gold: u32, rose: u32,
    pine: u32, foam: u32, iris: u32, hl_low: u32, hl_med: u32, hl_high: u32,
) -> Spec {
    Spec {
        name, kind, credit: &ROSE_PINE,
        bg: base, fg: text, caret: text, selection: hl_med, line: hl_low, gutter: muted,
        find: gold, guide: overlay, bracket: rose, accent: rose, whitespace: hl_high,
        comment: muted, string: gold, escape: pine, keyword: pine, operator: subtle,
        function: rose, ty: foam, constant: gold, number: gold, variable: text,
        parameter: iris, property: foam, punctuation: subtle, tag: foam, attribute: iris,
        macros: iris, heading: iris, link: iris, code: foam,
        inserted: foam, deleted: love, changed: rose, invalid: love,
        ansi: [love, pine, gold, foam, iris, rose], italic: IC | IP,
    }
}

const ROSE_PINE_MAIN: Spec = rose_pine(
    "Rosé Pine", Kind::Dark, 0x191724, 0x26233a, 0x6e6a86, 0x908caa, 0xe0def4, 0xeb6f92, 0xf6c177, 0xebbcba,
    0x31748f, 0x9ccfd8, 0xc4a7e7, 0x21202e, 0x403d52, 0x524f67,
);
const ROSE_PINE_MOON: Spec = rose_pine(
    "Rosé Pine Moon", Kind::Dark, 0x232136, 0x393552, 0x6e6a86, 0x908caa, 0xe0def4, 0xeb6f92, 0xf6c177, 0xea9a97,
    0x3e8fb0, 0x9ccfd8, 0xc4a7e7, 0x2a283e, 0x44415a, 0x56526e,
);
const ROSE_PINE_DAWN: Spec = rose_pine(
    "Rosé Pine Dawn", Kind::Light, 0xfaf4ed, 0xf2e9e1, 0x9893a5, 0x797593, 0x575279, 0xb4637a, 0xea9d34, 0xd7827e,
    0x286983, 0x56949f, 0x907aa9, 0xf4ede8, 0xdfdad9, 0xcecacd,
);

/// Everforest dark and light share the project's role mapping.
#[allow(clippy::too_many_arguments)]
const fn everforest(
    name: &'static str, kind: Kind,
    bg0: u32, bg1: u32, bg3: u32, bg4: u32, fg: u32, red: u32, orange: u32, yellow: u32, green: u32,
    aqua: u32, blue: u32, purple: u32, grey0: u32, grey1: u32,
) -> Spec {
    Spec {
        name, kind, credit: &EVERFOREST,
        bg: bg0, fg, caret: fg, selection: bg4, line: bg1, gutter: grey0,
        find: yellow, guide: bg3, bracket: orange, accent: green, whitespace: bg4,
        comment: grey1, string: aqua, escape: green, keyword: red, operator: orange,
        function: green, ty: yellow, constant: purple, number: purple, variable: fg,
        parameter: fg, property: blue, punctuation: grey1, tag: orange, attribute: yellow,
        macros: purple, heading: red, link: blue, code: green,
        inserted: green, deleted: red, changed: blue, invalid: red,
        ansi: [red, green, yellow, blue, purple, aqua], italic: IC,
    }
}

const EVERFOREST_DARK: Spec = everforest(
    "Everforest Dark", Kind::Dark, 0x2d353b, 0x343f44, 0x3d484d, 0x475258, 0xd3c6aa, 0xe67e80, 0xe69875, 0xdbbc7f,
    0xa7c080, 0x83c092, 0x7fbbb3, 0xd699b6, 0x7a8478, 0x859289,
);
const EVERFOREST_LIGHT: Spec = everforest(
    "Everforest Light", Kind::Light, 0xfdf6e3, 0xf4f0d9, 0xe6e2cc, 0xe0dcc7, 0x5c6a72, 0xf85552, 0xf57d26, 0xdfa000,
    0x8da101, 0x35a77c, 0x3a94c5, 0xdf69ba, 0xa6b0a0, 0x939f91,
);

const KANAGAWA_WAVE: Spec = Spec {
    name: "Kanagawa Wave", kind: Kind::Dark, credit: &KANAGAWA,
    bg: 0x1f1f28, fg: 0xdcd7ba, caret: 0xc8c093, selection: 0x2d4f67, line: 0x2a2a37, gutter: 0x54546d,
    find: 0xe6c384, guide: 0x2a2a37, bracket: 0xff9e3b, accent: 0x7e9cd8, whitespace: 0x363646,
    comment: 0x727169, string: 0x98bb6c, escape: 0xc0a36e, keyword: 0x957fb8, operator: 0xc0a36e,
    function: 0x7e9cd8, ty: 0x7aa89f, constant: 0xffa066, number: 0xd27e99, variable: 0xdcd7ba,
    parameter: 0xb8b4d0, property: 0xe6c384, punctuation: 0x9cabca, tag: 0x7e9cd8, attribute: 0xe6c384,
    macros: 0xe46876, heading: 0x7e9cd8, link: 0x7fb4ca, code: 0x98bb6c,
    inserted: 0x76946a, deleted: 0xc34043, changed: 0xdca561, invalid: 0xe82424,
    ansi: [0xc34043, 0x76946a, 0xc0a36e, 0x7e9cd8, 0x957fb8, 0x6a9589], italic: IC | IK,
};

const KANAGAWA_LOTUS: Spec = Spec {
    name: "Kanagawa Lotus", kind: Kind::Light, credit: &KANAGAWA,
    bg: 0xf2ecbc, fg: 0x545464, caret: 0x43436c, selection: 0xc9cbd1, line: 0xece4ae, gutter: 0x716e61,
    find: 0xcc6d00, guide: 0xe4d794, bracket: 0xcc6d00, accent: 0x4d699b, whitespace: 0xd5cea3,
    comment: 0x8a8980, string: 0x6f894e, escape: 0x836f4a, keyword: 0x624c83, operator: 0x836f4a,
    function: 0x4d699b, ty: 0x597b75, constant: 0xcc6d00, number: 0xb35b79, variable: 0x545464,
    parameter: 0x5d57a3, property: 0x77713f, punctuation: 0x4e8ca2, tag: 0x4d699b, attribute: 0x77713f,
    macros: 0xc84053, heading: 0x4d699b, link: 0x4e8ca2, code: 0x6f894e,
    inserted: 0x6e915f, deleted: 0xd7474b, changed: 0xde9800, invalid: 0xe82424,
    ansi: [0xc84053, 0x6f894e, 0x77713f, 0x4d699b, 0xb35b79, 0x597b75], italic: IC | IK,
};

const HC_DARK: Spec = Spec {
    name: "High Contrast Dark", kind: Kind::HighContrastDark, credit: &STRATA,
    bg: 0x000000, fg: 0xffffff, caret: 0xffd700, selection: 0x264f78, line: 0x1a1a1a, gutter: 0xc8c8c8,
    find: 0xffd700, guide: 0x6b6b6b, bracket: 0xffd700, accent: 0xffd700, whitespace: 0x6e6e6e,
    comment: 0xb8b8b8, string: 0xa6ff8f, escape: 0xffd84d, keyword: 0xff9df5, operator: 0xffffff,
    function: 0xffe066, ty: 0x4fe0ff, constant: 0xffa94d, number: 0xb4ff9f, variable: 0xffffff,
    parameter: 0xffd0a6, property: 0x8fd3ff, punctuation: 0xe6e6e6, tag: 0x4fe0ff, attribute: 0xffe066,
    macros: 0xff9df5, heading: 0xffe066, link: 0x8fd3ff, code: 0xa6ff8f,
    inserted: 0x7dff7d, deleted: 0xff8080, changed: 0xffd84d, invalid: 0xff6b6b,
    ansi: [0xff6b6b, 0x7dff7d, 0xffe066, 0x6cb6ff, 0xff9df5, 0x4fe0ff], italic: IC,
};

const HC_LIGHT: Spec = Spec {
    name: "High Contrast Light", kind: Kind::HighContrastLight, credit: &STRATA,
    bg: 0xffffff, fg: 0x000000, caret: 0x0b4fbf, selection: 0xadd6ff, line: 0xededed, gutter: 0x3a3a3a,
    find: 0xc78a00, guide: 0x9a9a9a, bracket: 0x0b4fbf, accent: 0x0b4fbf, whitespace: 0x8f8f8f,
    comment: 0x4f4f4f, string: 0x0e6b0e, escape: 0x9b0000, keyword: 0x8a0f8a, operator: 0x000000,
    function: 0x6f3d00, ty: 0x00578a, constant: 0x9a3500, number: 0x0b5f4f, variable: 0x000000,
    parameter: 0x5c2a00, property: 0x1f3f8a, punctuation: 0x1a1a1a, tag: 0x00578a, attribute: 0x6f3d00,
    macros: 0x8a0f8a, heading: 0x1f3f8a, link: 0x0b4fbf, code: 0x0e6b0e,
    inserted: 0x0e6b0e, deleted: 0x9b0000, changed: 0x6f3d00, invalid: 0x9b0000,
    ansi: [0xb00000, 0x0e6b0e, 0x6f4f00, 0x0b4fbf, 0x8a0f8a, 0x00606b], italic: IC,
};

/// Every curated scheme, in menu order within each kind.
pub static SPECS: &[Spec] = &[
    ONE_DARK, DRACULA_SPEC, NORD_SPEC, SOLARIZED_DARK, GRUVBOX_DARK, TOKYO_NIGHT, TOKYO_STORM, MOCHA, MACCHIATO,
    FRAPPE, GITHUB_DARK, MONOKAI_SPEC, AYU_DARK, AYU_MIRAGE, NIGHT_OWL_SPEC, PALENIGHT, ROSE_PINE_MAIN,
    ROSE_PINE_MOON, EVERFOREST_DARK, KANAGAWA_WAVE, //
    ONE_LIGHT, SOLARIZED_LIGHT, GRUVBOX_LIGHT, TOKYO_DAY, LATTE, GITHUB_LIGHT, AYU_LIGHT, LIGHT_OWL,
    ROSE_PINE_DAWN, EVERFOREST_LIGHT, KANAGAWA_LOTUS, //
    HC_DARK, HC_LIGHT,
];

/// Each palette source once, with the schemes taken from it (Help ▸ Licenses).
pub fn credits() -> Vec<(&'static Credit, Vec<&'static str>)> {
    let mut out: Vec<(&'static Credit, Vec<&'static str>)> = Vec::new();
    for spec in SPECS {
        match out.iter_mut().find(|(c, _)| std::ptr::eq(*c, spec.credit) || c.project == spec.credit.project) {
            Some((_, names)) => names.push(spec.name),
            None => out.push((spec.credit, vec![spec.name])),
        }
    }
    out
}

// ---- scope rules -----------------------------------------------------------------

/// A color role in the shared scope rules (also the keys of `theme_overrides`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Comment,
    String,
    Escape,
    Keyword,
    Operator,
    Function,
    Type,
    Constant,
    Number,
    Variable,
    Parameter,
    Property,
    Punctuation,
    Tag,
    Attribute,
    Macro,
    Heading,
    Link,
    Code,
    Inserted,
    Deleted,
    Changed,
    Invalid,
    /// `self`, `this`: the tag color, italic.
    LanguageVariable,
    Quote,
    ListMarker,
    DiffMeta,
}

impl Role {
    /// The setting key for overriding this role, if users can.
    pub fn key(self) -> Option<&'static str> {
        Some(match self {
            Role::Comment => "comment",
            Role::String => "string",
            Role::Escape => "escape",
            Role::Keyword => "keyword",
            Role::Operator => "operator",
            Role::Function => "function",
            Role::Type => "type",
            Role::Constant => "constant",
            Role::Number => "number",
            Role::Variable => "variable",
            Role::Parameter => "parameter",
            Role::Property => "property",
            Role::Punctuation => "punctuation",
            Role::Tag => "tag",
            Role::Attribute => "attribute",
            Role::Macro => "macro",
            Role::Heading => "heading",
            Role::Link => "link",
            Role::Code => "code",
            Role::Inserted => "inserted",
            Role::Deleted => "deleted",
            Role::Changed => "changed",
            Role::Invalid => "invalid",
            Role::LanguageVariable | Role::Quote | Role::ListMarker | Role::DiffMeta => return None,
        })
    }

    pub fn from_key(key: &str) -> Option<Role> {
        let key = match key {
            "decorator" | "annotation" | "macros" => "macro",
            "types" | "class" => "type",
            "keywords" => "keyword",
            "strings" => "string",
            "comments" => "comment",
            "functions" => "function",
            "constants" => "constant",
            "numbers" => "number",
            other => other,
        };
        RULES.iter().map(|r| r.0).find(|r| r.key() == Some(key))
    }
}

/// Scope selectors per role. Order doesn't decide anything: syntect prefers
/// the most specific selector, so `punctuation.definition.string` (string)
/// beats `punctuation` (punctuation).
pub static RULES: &[(Role, &str)] = &[
    (Role::Comment, "comment, punctuation.definition.comment"),
    (Role::String, "string, punctuation.definition.string, string.unquoted"),
    (Role::Escape, "constant.character.escape, string.regexp, constant.other.placeholder, constant.character.format, punctuation.definition.string.regexp"),
    (Role::Number, "constant.numeric, keyword.other.unit"),
    (Role::Constant, "constant, constant.language, constant.other, constant.character, support.constant, variable.other.constant, entity.name.constant, constant.other.color"),
    (Role::Keyword, "keyword, storage, storage.type, storage.modifier, keyword.control, keyword.declaration, keyword.operator.word, keyword.operator.new, keyword.other"),
    (Role::Operator, "keyword.operator, punctuation.separator.key-value, punctuation.accessor.arrow"),
    (Role::Variable, "variable, variable.other, variable.other.readwrite"),
    (Role::Parameter, "variable.parameter, meta.function.parameters variable.other"),
    (Role::LanguageVariable, "variable.language, support.variable.self"),
    (Role::Property, "variable.other.member, variable.other.property, variable.other.object.property, support.type.property-name, meta.object-literal.key, meta.mapping.key string, meta.property-name, entity.name.tag.yaml, string.unquoted.key, entity.name.label"),
    (Role::Function, "entity.name.function, support.function, variable.function, meta.function-call variable.function, entity.name.method"),
    (Role::Type, "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.interface, entity.name.union, entity.name.impl, entity.name.namespace, entity.name.module, support.type, support.class, entity.other.inherited-class, storage.type.primitive, storage.type.numeric, storage.type.boolean, storage.type.string"),
    (Role::Macro, "support.macro, entity.name.macro, meta.annotation, punctuation.definition.annotation, meta.decorator, variable.annotation, entity.name.function.decorator, punctuation.definition.decorator, meta.preprocessor, keyword.control.import.c, keyword.control.directive, punctuation.definition.attribute, meta.attribute.rust"),
    (Role::Punctuation, "punctuation, meta.brace, punctuation.section, punctuation.separator, punctuation.terminator, punctuation.accessor"),
    (Role::Tag, "entity.name.tag, punctuation.definition.tag, meta.tag.sgml"),
    (Role::Attribute, "entity.other.attribute-name, entity.other.attribute-name.class.css, entity.other.attribute-name.id.css"),
    (Role::Heading, "markup.heading, markup.heading entity.name, entity.name.section, punctuation.definition.heading"),
    (Role::Link, "markup.underline.link, string.other.link, meta.link.inline, constant.other.reference.link, meta.image.inline, markup.link"),
    (Role::Code, "markup.raw, markup.raw.inline, markup.raw.block, markup.inline.raw, markup.raw.code-fence"),
    (Role::Quote, "markup.quote"),
    (Role::ListMarker, "punctuation.definition.list_item, punctuation.definition.list.begin, markup.list.numbered.bullet, markup.list.unnumbered.bullet"),
    (Role::Inserted, "markup.inserted, meta.diff.header.to-file, punctuation.definition.inserted"),
    (Role::Deleted, "markup.deleted, meta.diff.header.from-file, punctuation.definition.deleted"),
    (Role::Changed, "markup.changed, punctuation.definition.changed"),
    (Role::DiffMeta, "meta.diff.range, meta.diff.header, meta.separator.diff"),
    (Role::Invalid, "invalid, invalid.illegal, message.error"),
];

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio, 1.0 to 21.0.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// `c`, moved toward white (dark backgrounds) or black (light ones) just far
/// enough to reach `min` contrast against `bg`.
pub fn ensure_contrast(c: Color32, bg: Color32, min: f32) -> Color32 {
    if contrast(c, bg) >= min {
        return c;
    }
    let target = if luminance(bg) > 0.18 { Color32::BLACK } else { Color32::WHITE };
    (1..=50).map(|i| blend(c, target, i as f32 * 0.02)).find(|&x| contrast(x, bg) >= min).unwrap_or(target)
}

pub fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

impl Spec {
    /// The palette color for a role, before legibility adjustments.
    pub fn role(&self, role: Role) -> u32 {
        match role {
            Role::Comment => self.comment,
            Role::String => self.string,
            Role::Escape => self.escape,
            Role::Keyword => self.keyword,
            Role::Operator => self.operator,
            Role::Function => self.function,
            Role::Type => self.ty,
            Role::Constant => self.constant,
            Role::Number => self.number,
            Role::Variable => self.variable,
            Role::Parameter => self.parameter,
            Role::Property => self.property,
            Role::Punctuation => self.punctuation,
            Role::Tag | Role::LanguageVariable => self.tag,
            Role::Attribute => self.attribute,
            Role::Macro | Role::DiffMeta => self.macros,
            Role::Heading => self.heading,
            Role::Link => self.link,
            Role::Code => self.code,
            Role::Quote => self.parameter,
            Role::ListMarker => self.operator,
            Role::Inserted => self.inserted,
            Role::Deleted => self.deleted,
            Role::Changed => self.changed,
            Role::Invalid => self.invalid,
        }
    }

    fn font_style(&self, role: Role) -> FontStyle {
        let flag = |f: u8| self.italic & f != 0;
        match role {
            Role::Comment if flag(IC) => FontStyle::ITALIC,
            Role::Keyword if flag(IK) => FontStyle::ITALIC,
            Role::Type if flag(IT) => FontStyle::ITALIC,
            Role::Parameter if flag(IP) => FontStyle::ITALIC,
            Role::Attribute if flag(IA) => FontStyle::ITALIC,
            Role::LanguageVariable | Role::Quote => FontStyle::ITALIC,
            Role::Heading => FontStyle::BOLD,
            Role::Link => FontStyle::UNDERLINE,
            _ => FontStyle::empty(),
        }
    }
}

/// The minimum contrast for text and for syntax colors.
pub fn minimums(kind: Kind) -> (f32, f32) {
    if kind.is_high_contrast() { (7.0, 4.5) } else { (4.5, 3.0) }
}

fn to_syntect(c: Color32) -> syntect::highlighting::Color {
    syntect::highlighting::Color { r: c.r(), g: c.g(), b: c.b(), a: c.a() }
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Build a full scheme from a palette.
pub fn build(spec: &Spec) -> Scheme {
    let bg = hex(spec.bg);
    let (text_min, syntax_min) = minimums(spec.kind);
    let fg = ensure_contrast(hex(spec.fg), bg, text_min);
    let c = |v: u32| to_syntect(ensure_contrast(hex(v), bg, syntax_min));
    let settings = ThemeSettings {
        foreground: Some(to_syntect(fg)),
        background: Some(to_syntect(bg)),
        caret: Some(to_syntect(hex(spec.caret))),
        line_highlight: Some(to_syntect(hex(spec.line))),
        selection: Some(to_syntect(hex(spec.selection))),
        inactive_selection: Some(to_syntect(blend(bg, hex(spec.selection), 0.6))),
        gutter: Some(to_syntect(bg)),
        gutter_foreground: Some(to_syntect(hex(spec.gutter))),
        find_highlight: Some(to_syntect(hex(spec.find))),
        guide: Some(to_syntect(hex(spec.guide))),
        active_guide: Some(to_syntect(blend(hex(spec.guide), fg, 0.25))),
        stack_guide: Some(to_syntect(hex(spec.guide))),
        brackets_foreground: Some(to_syntect(hex(spec.bracket))),
        highlight: Some(to_syntect(with_alpha(fg, 0x55))),
        accent: Some(to_syntect(hex(spec.accent))),
        misspelling: Some(to_syntect(hex(spec.invalid))),
        minimap_border: Some(to_syntect(hex(spec.guide))),
        ..Default::default()
    };
    let mut scopes: Vec<ThemeItem> = RULES
        .iter()
        .map(|&(role, selectors)| ThemeItem {
            scope: ScopeSelectors::from_str(selectors).expect("built-in selectors parse"),
            style: StyleModifier { foreground: Some(c(spec.role(role))), background: None, font_style: Some(spec.font_style(role)) },
        })
        .collect();
    for (sel, style) in [("markup.bold", FontStyle::BOLD), ("markup.italic", FontStyle::ITALIC), ("markup.underline", FontStyle::UNDERLINE)] {
        scopes.push(ThemeItem {
            scope: ScopeSelectors::from_str(sel).expect("built-in selectors parse"),
            style: StyleModifier { foreground: None, background: None, font_style: Some(style) },
        });
    }
    let theme = Theme { name: Some(spec.name.into()), author: Some(spec.credit.author.into()), settings, scopes };
    Scheme {
        theme,
        kind: spec.kind,
        whitespace: Some(rgb_color(hex(spec.whitespace))),
        ansi: Some(spec.ansi.map(|v| ensure_contrast(hex(v), bg, syntax_min))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_each_kind_is_represented() {
        let mut names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n, "duplicate scheme names");
        for kind in [Kind::Dark, Kind::Light, Kind::HighContrastDark, Kind::HighContrastLight] {
            assert!(SPECS.iter().any(|s| s.kind == kind), "{kind:?}");
        }
        assert!(SPECS.iter().filter(|s| s.kind == Kind::Dark).count() >= 15);
        assert!(SPECS.iter().filter(|s| s.kind == Kind::Light).count() >= 10);
    }

    #[test]
    fn every_role_has_selectors_and_keys_round_trip() {
        for (role, sel) in RULES {
            assert!(ScopeSelectors::from_str(sel).is_ok(), "{role:?}");
            if let Some(key) = role.key() {
                assert_eq!(Role::from_key(key), Some(*role));
            }
        }
        assert_eq!(Role::from_key("decorator"), Some(Role::Macro));
        assert_eq!(Role::from_key("nope"), None);
    }

    #[test]
    fn kinds_match_the_background() {
        for s in SPECS {
            let light = luminance(hex(s.bg)) > 0.3;
            assert_eq!(light, s.kind.is_light(), "{}", s.name);
        }
    }

    #[test]
    fn contrast_math_matches_wcag() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(hex(0x777777), Color32::WHITE) - 4.48).abs() < 0.02);
        let fixed = ensure_contrast(hex(0x404040), Color32::BLACK, 4.5);
        assert!(contrast(fixed, Color32::BLACK) >= 4.5);
    }

    /// Before any nudging, text in every palette is already AA; high-contrast
    /// palettes meet AAA text and AA syntax colors as written.
    #[test]
    fn palettes_are_legible_as_written() {
        for s in SPECS {
            let bg = hex(s.bg);
            let fg = contrast(hex(s.fg), bg);
            assert!(fg >= 4.5, "{}: foreground contrast {fg:.2}", s.name);
            if s.kind.is_high_contrast() {
                assert!(fg >= 7.0, "{}: {fg:.2}", s.name);
                for (role, _) in RULES {
                    let c = contrast(hex(s.role(*role)), bg);
                    assert!(c >= 4.5, "{}: {role:?} contrast {c:.2}", s.name);
                }
                let on_sel = contrast(hex(s.fg), hex(s.selection));
                assert!(on_sel >= 4.5, "{}: text on selection {on_sel:.2}", s.name);
            }
        }
    }
}
