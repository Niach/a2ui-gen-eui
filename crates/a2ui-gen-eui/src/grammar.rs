//! A catalog's JSON schema to a grammar of the Express a model may write for it.
//!
//! The grammar pins down the frame (`<a2ui>`, `surface("main")`, data lines, `root = …` as the first
//! component, one statement per line, `</a2ui>`), per component the property names, per property the
//! values its schema allows, and the function names. It is the canonical spelling the Express decompiler
//! writes (`name = Component(key=value, …)`). Being context-free, it cannot check ids: that a referenced
//! id is defined, once, is left to the compiler ([`crate::Catalog::validate`]).
//!
//! Two spellings of the same grammar: [`lark`] for llguidance, which this crate decodes with, and [`gbnf`]
//! for llama.cpp, vLLM (xgrammar) and others.
//!
//! A catalog that declares a2ui-express's `at_exponential_express` extension also gets its forms
//! (`docs/express-extensions.md` there), generated from the tables its compiler reads: a `theme(...)`
//! line before `root` (`#rrggbb[aa]` colours only there), `style(name, "words")` classes, `s=` on every
//! component, `enter=` / `exit=` / `stagger=` / `delay=`, Motion `ref=` (over the catalog's registry) and
//! `kf=`, and [`lark_patch`] / [`gbnf_patch`] for patches (`del(name)`, `del($/path)`). A catalog that
//! does not declare it gets the same grammar as before, byte for byte.

use std::collections::HashMap;

use a2ui_express::ext::{self, motion, style, theme};
use serde_json::Value;

const COMMON: &str = "common_types.json#/$defs/";

const SHARED: &str = r#"
call ::= fname "(" (arg ("," ws arg)*)? ")"
arg ::= (ident "=")? any
anylist ::= "[" (any ("," ws any)*)? "]"
anymap ::= "{" (key ":" ws any ("," ws key ":" ws any)*)? "}"
any ::= string | number | boolean | "null" | path | event | template | call | anylist | anymap | ident | check
check ::= "?" fname ("(" (arg ("," ws arg)*)? ")")?
dataval ::= string | number | boolean | "null" | path | event | template | call | datalist | datamap | check
datalist ::= "[" (dataval ("," ws dataval)*)? "]"
datamap ::= "{" (key ":" ws dataval ("," ws key ":" ws dataval)*)? "}"
checks ::= check | "[" (check ("," ws check)*)? "]"
event ::= "Event(" string ("," ws anymap)? ("," ws "userMessage=" (string | path | call))? ")"
action ::= event | call | "[" (event | call) ("," ws (event | call))* "]" | anymap
idref ::= ident | string
idlist ::= "[" (idref ("," ws idref)*)? "]"
template ::= "_template(" path "," ws ident ("," ws ident "=" any)* ")"
children ::= idlist | template
key ::= ident | string
boolean ::= "true" | "false"
ws ::= " "
"#;

const GBNF_TOKENS: &str = r#"
ident ::= ([A-Za-z_] | [^\x00-\x7F]) ([A-Za-z0-9_] | [^\x00-\x7F])*
string ::= "\"\"\"" tchar* "\"\"\"" | "r\"\"\"" rtchar* "\"\"\"" | "r\"" [^"\n]* "\"" | "\"" char* "\""
rtchar ::= [^"] | "\"" [^"] | "\"\"" [^"]
char ::= [^"\\\n] | "\\" [^\n]
tchar ::= [^"\\] | "\\" [^\n] | "\"" [^"\\] | "\"\"" [^"\\]
number ::= "-"? [0-9]+ ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
path ::= "$" [A-Za-z0-9_/]*
abspath ::= "$/" | "$" ("/" [A-Za-z0-9_]+)+
"#;

// Lengths are bounded, so a model caught in a loop cannot spend a whole reply on one name or number.
// The lexer matches greedily, so a string is one terminal for all four forms.
const LARK_TOKENS: &str = r#"
ident: IDENT
string: STRING
number: NUMBER
path: PATH
abspath: ABSPATH
IDENT: /[\p{XID_Start}_][\p{XID_Continue}]{0,255}/
STRING: /"""([^"\\]|\\[^\n]|"[^"\\]|""[^"\\])*"""|r"""([^"]|"[^"]|""[^"])*"""|r"[^"\n]*"|"([^"\\\n]|\\[^\n])*"/
NUMBER: /-?[0-9]{1,20}(\.[0-9]{1,20})?([eE][-+]?[0-9]{1,3})?/
PATH: /\$(\/?[A-Za-z0-9_]{1,64}(\/[A-Za-z0-9_]{1,64}){0,7}|\/)?/
ABSPATH: /\$(\/|(\/[A-Za-z0-9_]{1,64}){1,8})/
"#;

#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Gbnf,
    Lark,
}

/// The grammar for llama.cpp's GBNF sampler and compatible engines. The start rule is `root`.
pub fn gbnf(catalog: &Value) -> String {
    Builder::new(catalog, Dialect::Gbnf).build(false)
}

/// The grammar in llguidance's Lark dialect. The start rule is `start`.
pub fn lark(catalog: &Value) -> String {
    Builder::new(catalog, Dialect::Lark).build(false)
}

/// [`gbnf`] for a patch: `<a2ui>`, then changes to a screen that exists (no `surface(...)`, no `root`
/// first: reassignments, data paths, `style(...)`, `theme(...)`, `del(...)`), then `</a2ui>`. `None`
/// when the catalog does not declare the `at_exponential_express` extension, which patches need.
pub fn gbnf_patch(catalog: &Value) -> Option<String> {
    let b = Builder::new(catalog, Dialect::Gbnf);
    b.ext.is_some().then(|| b.build(true))
}

/// [`lark`] for a patch; see [`gbnf_patch`].
pub fn lark_patch(catalog: &Value) -> Option<String> {
    let b = Builder::new(catalog, Dialect::Lark);
    b.ext.is_some().then(|| b.build(true))
}

/// What a catalog declares of the `at_exponential_express` extension.
struct Ext {
    /// The `Motion(ref=…)` registry's names.
    motions: Vec<String>,
}

impl Ext {
    fn of(catalog: &Value) -> Option<Ext> {
        let decl = catalog.pointer(&format!("/metadata/extensions/{}", ext::EXTENSION))?;
        let motions = match decl {
            Value::Bool(true) => Vec::new(),
            Value::Object(o) => o
                .get("motions")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            _ => return None,
        };
        Some(Ext { motions })
    }
}

struct Builder<'a> {
    catalog: &'a Value,
    dialect: Dialect,
    ext: Option<Ext>,
    rules: Vec<(String, String)>,
    /// Rule body (or `ref:<name>`) to rule name.
    memo: HashMap<String, String>,
}

/// A terminal for `text`. GBNF and Lark both read JSON string escapes.
fn lit(text: &str) -> String {
    Value::from(text).to_string()
}

impl<'a> Builder<'a> {
    fn new(catalog: &'a Value, dialect: Dialect) -> Self {
        Builder {
            catalog,
            dialect,
            ext: Ext::of(catalog),
            rules: Vec::new(),
            memo: HashMap::new(),
        }
    }

    fn rule(&mut self, body: String, hint: &str) -> String {
        if let Some(name) = self.memo.get(&body) {
            return name.clone();
        }
        let name = self.name(hint);
        self.memo.insert(body.clone(), name.clone());
        if self.dialect == Dialect::Lark && hint == "enum" {
            // One terminal per enum: eui's icon enum alone has hundreds of values.
            let token = name.to_uppercase();
            self.rules.push((name.clone(), token.clone()));
            self.rules.push((token, body));
        } else {
            self.rules.push((name.clone(), body));
        }
        name
    }

    fn name(&self, hint: &str) -> String {
        format!("{}{}", hint.to_lowercase(), self.memo.len() + 1)
    }

    fn enumeration(&mut self, values: &[Value]) -> String {
        // The literal is the value's JSON spelling, itself quoted as a terminal.
        let alts: Vec<String> = values.iter().map(|v| lit(&v.to_string())).collect();
        self.rule(alts.join(" | "), "enum")
    }

    fn union(&mut self, parts: Vec<String>) -> String {
        let mut seen: Vec<String> = Vec::new();
        for p in parts {
            if !seen.contains(&p) {
                seen.push(p);
            }
        }
        if seen.iter().any(|p| p == "any") {
            return "any".into();
        }
        match seen.len() {
            0 => "any".into(),
            1 => seen.remove(0),
            _ => self.rule(seen.join(" | "), "u"),
        }
    }

    fn strs(&mut self, parts: &[&str]) -> String {
        self.union(parts.iter().map(|p| (*p).to_owned()).collect())
    }

    /// The rule a schema's values match.
    fn conv(&mut self, schema: &Value) -> String {
        let Some(s) = schema.as_object() else {
            return "any".into();
        };
        if let Some(r) = s.get("$ref").and_then(Value::as_str) {
            return self.reference(r);
        }
        for k in ["oneOf", "anyOf"] {
            if let Some(alts) = s.get(k).and_then(Value::as_array) {
                let parts = alts.iter().map(|x| self.conv(x)).collect();
                return self.union(parts);
            }
        }
        if let Some(parts) = s.get("allOf").and_then(Value::as_array) {
            return match parts.as_slice() {
                [only] => self.conv(only),
                _ => "any".into(),
            };
        }
        if let Some(c) = s.get("const") {
            return self.enumeration(std::slice::from_ref(c));
        }
        if let Some(values) = s.get("enum").and_then(Value::as_array) {
            return self.enumeration(values);
        }
        match s.get("type") {
            Some(Value::Array(types)) => {
                let parts = types
                    .iter()
                    .map(|t| {
                        let mut one = s.clone();
                        one.insert("type".into(), t.clone());
                        self.conv(&Value::Object(one))
                    })
                    .collect();
                self.union(parts)
            }
            Some(Value::String(t)) => match t.as_str() {
                "string" => "string".into(),
                "number" | "integer" => "number".into(),
                "boolean" => "boolean".into(),
                "null" => self.rule("\"null\"".into(), "null"),
                "array" => match s.get("items") {
                    Some(Value::Object(items)) if !items.is_empty() => {
                        let item = self.conv(&Value::Object(items.clone()));
                        if item == "idref" {
                            return "idlist".into();
                        }
                        self.rule(format!("\"[\" ({item} (\",\" ws {item})*)? \"]\""), "list")
                    }
                    _ => "anylist".into(),
                },
                "object" => self.map(s),
                _ => "any".into(),
            },
            _ if s.contains_key("properties") => self.map(s),
            _ => "any".into(),
        }
    }

    fn map(&mut self, s: &serde_json::Map<String, Value>) -> String {
        let empty = serde_json::Map::new();
        let props = s.get("properties").and_then(Value::as_object).unwrap_or(&empty);
        let extra = s.get("additionalProperties");
        if props.is_empty() && !extra.is_some_and(Value::is_object) {
            return "anymap".into();
        }
        let mut alts: Vec<String> = props
            .iter()
            .map(|(k, v)| format!("{} {}", lit(&format!("{k}: ")), self.conv(v)))
            .collect();
        match extra {
            Some(e) if e.is_object() => alts.push(format!("key \":\" ws {}", self.conv(e))),
            Some(Value::Bool(false)) => {}
            _ if s.get("unevaluatedProperties") == Some(&Value::Bool(false)) => {}
            _ => alts.push("key \":\" ws any".into()),
        }
        let kv = self.rule(alts.join(" | "), "kv");
        self.rule(format!("\"{{\" ({kv} (\",\" ws {kv})*)? \"}}\""), "map")
    }

    fn reference(&mut self, r: &str) -> String {
        if let Some(name) = r.strip_prefix(COMMON) {
            return match name {
                "DynamicString" => self.strs(&["string", "path", "call"]),
                "DynamicNumber" => self.strs(&["number", "path", "call"]),
                "DynamicBoolean" => self.strs(&["boolean", "path", "call"]),
                "DynamicStringList" => {
                    let list = self.rule("\"[\" (string (\",\" ws string)*)? \"]\"".into(), "strs");
                    self.strs(&[&list, "path", "call"])
                }
                "DataBinding" => "path".into(),
                "FunctionCall" => "call".into(),
                "ComponentId" | "Child" => "idref".into(),
                "ChildList" => "children".into(),
                "Action" => "action".into(),
                "CheckRule" => "check".into(),
                _ => "any".into(),
            };
        }
        let name = r.rsplit('/').next().unwrap_or(r);
        match name {
            "EuiAction" => return "action".into(),
            "ChildList" => return "children".into(),
            "check" => return "check".into(),
            _ => {}
        }
        let Some(target) = self.catalog.get("$defs").and_then(|d| d.get(name)) else {
            return "any".into();
        };
        let key = format!("ref:{name}");
        if let Some(rule) = self.memo.get(&key) {
            return rule.clone();
        }
        // Named before it is converted: types may be recursive (menuItem.items, treeItem.children).
        let hint: String = name.chars().filter(char::is_ascii_alphanumeric).collect();
        let rule = self.name(&format!("t{hint}x"));
        self.memo.insert(key, rule.clone());
        let body = self.conv(target);
        self.rules.push((rule.clone(), body));
        rule
    }

    fn component(&mut self, name: &str, schema: &Value) -> String {
        let mut props = schema
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut checkable = false;
        for part in schema
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if part
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|r| r.ends_with("Checkable"))
            {
                checkable = true;
            } else if let Some(more) = part.get("properties").and_then(Value::as_object) {
                props.extend(more.clone());
            }
        }
        let mut alts = Vec::new();
        for (p, s) in &props {
            match p.as_str() {
                "component" => {}
                // the check rules' envelope key, written inline; Express spells it `checks=`
                "@checks" => checkable = true,
                p if p.starts_with('@') => {}
                "style" => alts.push("\"style=\" anymap".into()),
                _ => alts.push(format!("{} {}", lit(&format!("{p}=")), self.conv(s))),
            }
        }
        // a `checks` property of the component's own is an ordinary one, with the schema it declares
        if checkable && !props.contains_key("checks") {
            alts.push("\"checks=\" checks".into());
        }
        // the extension's keywords, where the component does not declare a property of that name
        if let Some(motions) = self.ext.as_ref().map(|e| e.motions.clone()) {
            let free = |k: &str| !props.contains_key(k);
            if free("s") {
                alts.push("\"s=\" xsval".into());
            }
            for (kw, _) in motion::KEYS {
                if !free(kw) {
                    continue;
                }
                let value = match *kw {
                    "enter" => self.names(motion::ENTER),
                    "exit" => self.names(motion::EXIT),
                    _ => "XUNUM".into(),
                };
                alts.push(format!("{} {value}", lit(&format!("{kw}="))));
            }
            if name == "Motion" {
                if free("ref") && !motions.is_empty() {
                    let mut sorted = motions;
                    sorted.sort();
                    let names: Vec<&str> = sorted.iter().map(String::as_str).collect();
                    let value = self.names(&names);
                    alts.push(format!("\"ref=\" {value}"));
                }
                if free("kf") {
                    alts.push("\"kf=\" XKF".into());
                }
            }
        }
        if alts.is_empty() {
            alts.push("\"visible=\" any".into());
        }
        let kw = self.rule(alts.join(" | "), "kw");
        format!("{} ({kw} (\",\" ws {kw})*)? \")\"", lit(&format!("{name}(")))
    }

    /// An enum of string values.
    fn names(&mut self, names: &[&str]) -> String {
        let values: Vec<Value> = names.iter().map(|n| Value::from(*n)).collect();
        self.enumeration(&values)
    }

    fn build(mut self, patch: bool) -> String {
        let empty = serde_json::Map::new();
        let catalog = self.catalog;
        let object = |key: &str| catalog.get(key).and_then(Value::as_object).unwrap_or(&empty);
        let comps: Vec<String> = object("components")
            .iter()
            .map(|(n, s)| {
                let body = self.component(n, s);
                self.rule(body, "c")
            })
            .collect();
        let mut names: Vec<&String> = object("functions").keys().collect();
        names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
        let mut fnames: Vec<String> = names.into_iter().map(|n| lit(n)).collect();
        fnames.push("\"@index\"".into());

        let start = match self.dialect {
            Dialect::Gbnf => "root",
            Dialect::Lark => "start",
        };
        let frame = match (&self.ext, patch) {
            (None, _) => format!(
                "{start} ::= {} (data \"\\n\")* \"root = \" component \"\\n\" (stmt \"\\n\")* \"</a2ui>\"\n\
                 stmt ::= data | ident \" = \" component\n",
                lit(crate::prompt::REPLY_START)
            ),
            // a theme is part of the surface it creates, so it comes before `root`; a class before its first use
            (Some(_), false) => format!(
                "{start} ::= {} (pre \"\\n\")* \"root = \" component \"\\n\" (stmt \"\\n\")* \"</a2ui>\"\n\
                 pre ::= data | xtheme | xstyle\n\
                 stmt ::= data | ident \" = \" component | xstyle\n",
                lit(crate::prompt::REPLY_START)
            ),
            (Some(_), true) => format!(
                "{start} ::= {} (stmt \"\\n\")+ \"</a2ui>\"\n\
                 stmt ::= data | ident \" = \" component | xtheme | xstyle | xdel\n",
                lit("<a2ui>\n")
            ),
        };
        let mut out = format!(
            "{frame}\
             data ::= abspath \" = \" dataval\n\
             component ::= {}\n\
             fname ::= {}\n",
            comps.join(" | "),
            fnames.join(" | ")
        );
        out.push_str(SHARED.trim_start());
        for (name, body) in &self.rules {
            out.push_str(&format!("{name} ::= {body}\n"));
        }
        if self.ext.is_some() {
            out.push_str(&express_rules(self.dialect));
        }
        match self.dialect {
            Dialect::Gbnf => out + GBNF_TOKENS.trim_start(),
            Dialect::Lark => {
                let rules = out
                    .lines()
                    .map(|l| l.replacen(" ::= ", ": ", 1) + "\n")
                    .collect::<String>();
                format!("%llguidance {{}}\n{rules}{}", LARK_TOKENS.trim_start())
            }
        }
    }
}

// ---- the at_exponential_express extension ----------------------------------------------------------

/// What a theme seed (or alias, or extra key) takes.
fn seed_value(name: &str) -> &'static str {
    let name = theme::ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, seed)| seed);
    match name {
        "base" => "xmode",
        "bg" | "fg" | "card" | "primary" | "secondary" | "muted" | "accent" | "border" => "xcolor",
        "charts" => "xcharts",
        "radius" | "scale" | "stroke" => "XUNUM",
        "font" | "display" | "id" | "name" | "extends" => "string",
        "density" => "xdensity",
        "shadow" => "xshadow",
        "button" => "xbutton",
        "light" | "dark" => "xcolors",
        "tokens" | "fonts" | "recipes" => "xtmap",
        _ if theme::ALL_COLORS.contains(&name) => "xcolor",
        _ => "xtval",
    }
}

/// The values of the theme seeds that are a choice. `ext/theme.rs` keeps them private; the tests check
/// the compiler takes each and refuses others.
pub(crate) const THEME_CHOICES: &[(&str, &[&str])] = &[
    ("xmode", &["light", "dark"]),
    ("xdensity", &["compact", "default", "comfortable"]),
    ("xshadow", &["none", "soft", "strong"]),
    ("xbutton", &["rounded", "pill", "square"]),
];

/// The keyframe text's eases (`ext/motion.rs`, private there; the tests compile each).
pub(crate) const KF_EASES: &[&str] = &["linear", "ease", "ease-in", "ease-out", "ease-in-out", "hold"];

fn alts(items: impl IntoIterator<Item = String>) -> String {
    let mut seen: Vec<String> = Vec::new();
    for i in items {
        if !seen.contains(&i) {
            seen.push(i);
        }
    }
    seen.join(" | ")
}

/// One style shorthand word without a state prefix, over `ext::style`'s table: the bare words, then
/// per key its prefix and what may follow it. A prefix names the first key that has it, as the
/// compiler's lookup does.
fn style_word() -> String {
    let mut out: Vec<String> = style::bare_words().map(|(w, _, _)| lit(w)).collect();
    let mut by_rest: Vec<(&str, Vec<String>)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for k in style::KEYS {
        if seen.contains(&k.prefix) {
            continue;
        }
        seen.push(k.prefix);
        let grouped = k.group.is_some();
        let rest = match k.kind {
            style::Kind::Enum(values) => {
                let words = alts(values.iter().flat_map(|v| [lit(v.short), lit(v.value)]));
                out.push(format!("{} ({words})", lit(&format!("{}-", k.prefix))));
                continue;
            }
            style::Kind::Length if grouped => "XRLENG",
            style::Kind::Length => "XRLENN",
            style::Kind::Number if grouped => "XRNUMG",
            style::Kind::Number => "XRNUMN",
            style::Kind::Token if grouped => "XRTOKG",
            style::Kind::Token => "XRTOKN",
        };
        match by_rest.iter_mut().find(|(r, _)| *r == rest) {
            Some((_, prefixes)) => prefixes.push(lit(k.prefix)),
            None => by_rest.push((rest, vec![lit(k.prefix)])),
        }
    }
    for (rest, prefixes) in by_rest {
        out.push(format!("({}) {rest}", prefixes.join(" | ")));
    }
    alts(out)
}

/// The extension's rules, in either dialect. Upper-case names are terminals in Lark (a whole string of
/// style words or keyframes is one lexeme); in GBNF they are rules like the rest.
fn express_rules(dialect: Dialect) -> String {
    let lark = dialect == Dialect::Lark;
    let states = alts(style::STATES.iter().map(|(s, _)| lit(s)));
    let mut seed_names: Vec<&str> = Vec::new();
    for n in theme::SEEDS
        .iter()
        .chain(theme::ALIASES.iter().map(|(a, _)| a))
        .chain(theme::EXTRA)
        .chain(theme::ALL_COLORS)
    {
        if !seed_names.contains(n) {
            seed_names.push(n);
        }
    }
    let targ = alts(
        seed_names
            .iter()
            .map(|n| format!("{} {}", lit(&format!("{n}=")), seed_value(n))),
    );
    let cname = alts(theme::ALL_COLORS.iter().map(|c| lit(c)));
    let mut r: Vec<(String, String)> = vec![
        (
            "xtheme".into(),
            "\"theme(\" (string | xtarg (\",\" ws xtarg)*) \")\"".into(),
        ),
        ("xtarg".into(), targ),
        // at most 8
        (
            "xcharts".into(),
            format!("\"[\" xcolor{} \"]\"", " (\",\" ws xcolor)?".repeat(7)),
        ),
        // a colour literal, or the same in quotes; colour literals are valid nowhere else
        ("xcolor".into(), "XHEX | XQHEX".into()),
        ("XQHEX".into(), "\"\\\"\" XHEX \"\\\"\"".into()),
        ("xcolors".into(), "\"{\" xckv (\",\" ws xckv)* \"}\"".into()),
        ("xckv".into(), "xcname \":\" ws xcolor".into()),
        ("xcname".into(), cname),
        (
            "xtval".into(),
            "XHEX | string | number | boolean | \"null\" | xtlist | xtmap".into(),
        ),
        ("xtlist".into(), "\"[\" (xtval (\",\" ws xtval)*)? \"]\"".into()),
        (
            "xtmap".into(),
            "\"{\" (key \":\" ws xtval (\",\" ws key \":\" ws xtval)*)? \"}\"".into(),
        ),
    ];
    for (name, values) in THEME_CHOICES {
        r.push((
            (*name).into(),
            alts(values.iter().map(|v| lit(&format!("\"{v}\"")))),
        ));
    }
    r.extend([
        ("xstyle".into(), "\"style(\" XCLASS \",\" ws XSWORDS \")\"".into()),
        ("xdel".into(), "\"del(\" (ident | abspath) \")\"".into()),
        ("xsval".into(), "XSWORDS | XCLASS".into()),
        ("XSWORDS".into(), "\"\\\"\" XSITEM (\" \" XSITEM)* \"\\\"\"".into()),
        ("XSITEM".into(), "XSWORD | XCLASS".into()),
        ("XSWORD".into(), format!("(({states}) \":\")? XSBASE")),
        ("XSBASE".into(), style_word()),
        ("XRLENG".into(), "XNUM (\"%\" | \"px\")? | \"-auto\" | \"-\" XTOKNAME".into()),
        ("XRLENN".into(), "XNUM (\"%\" | \"px\")? | \"-auto\" | \"-\" XTOKDOT".into()),
        ("XRNUMG".into(), "XNUM | \"-\" XTOKNAME".into()),
        ("XRNUMN".into(), "XNUM | \"-\" XTOKDOT".into()),
        ("XRTOKG".into(), "\"-\" XTOKNAME".into()),
        ("XRTOKN".into(), "\"-\" XTOKDOT".into()),
        ("XTOKNAME".into(), "XTOKPART (\".\" XTOKPART)*".into()),
        ("XTOKDOT".into(), "XTOKPART (\".\" XTOKPART)+".into()),
        ("XUNUM".into(), "XDIGITS (\".\" XDIGITS)?".into()),
        ("XNUM".into(), "\"-\"? XUNUM".into()),
        ("XKF".into(), "\"\\\"\" XKLAYER (XKS? \";\" XKS? XKLAYER)* \"\\\"\"".into()),
        ("XKLAYER".into(), "XKSHAPE (XKS? \"|\" XKS? XKTRACK)*".into()),
        (
            "XKSHAPE".into(),
            [("circle", 3), ("ring", 4), ("rect", 5), ("star", 5)]
                .iter()
                .map(|(shape, n)| format!("{}{} XKS XKCOLOR", lit(shape), " XKS XNUM".repeat(*n)))
                .collect::<Vec<_>>()
                .join(" | "),
        ),
        (
            "XKTRACK".into(),
            "(\"scale\" | \"opacity\" | \"rotate\") XKS XKKEY (XKS XKKEY)+ (XKS XKEASE)? | \"move\" XKS XKMOVE (XKS XKMOVE)+ (XKS XKEASE)?".into(),
        ),
        ("XKKEY".into(), "XUNUM \":\" XNUM".into()),
        ("XKMOVE".into(), "XUNUM \":\" XNUM \",\" XNUM".into()),
        ("XKEASE".into(), alts(KF_EASES.iter().map(|e| lit(e)))),
        ("XKCOLOR".into(), "\"$color.\" XKTOK | XKHEX".into()),
        ("XKS".into(), "\" \"+".into()),
    ]);
    // the character classes, which the dialects spell differently
    if lark {
        r.extend([
            ("XCLASS".into(), format!("IDENT & ~(XSBASE | {states})")),
            ("XDIGITS".into(), "/[0-9]{1,20}/".into()),
            ("XTOKPART".into(), "/[A-Za-z][A-Za-z0-9]{0,63}/".into()),
            ("XKTOK".into(), "/[A-Za-z0-9]{1,64}/".into()),
            ("XHEX".into(), "/#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?/".into()),
            ("XKHEX".into(), "/#[0-9a-f]{6}([0-9a-f]{2})?/".into()),
        ]);
    } else {
        let hex = |class: &str| {
            format!(
                "\"#\"{} ({})?",
                format!(" {class}").repeat(6),
                [class; 2].join(" ")
            )
        };
        r.extend([
            // GBNF has no complement: a class name here is any identifier, the compiler refuses one
            // that reads as a word
            ("XCLASS".into(), "ident".into()),
            ("XDIGITS".into(), "[0-9]+".into()),
            ("XTOKPART".into(), "[A-Za-z] [A-Za-z0-9]*".into()),
            ("XKTOK".into(), "[A-Za-z0-9]+".into()),
            ("XHEX".into(), hex("[0-9a-fA-F]")),
            ("XKHEX".into(), hex("[0-9a-f]")),
        ]);
    }
    r.into_iter()
        .map(|(name, body)| format!("{name} ::= {body}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seed_has_its_own_values() {
        for seed in theme::SEEDS
            .iter()
            .chain(theme::EXTRA)
            .chain(theme::ALIASES.iter().map(|(a, _)| a))
        {
            assert_ne!(seed_value(seed), "xtval", "{seed}");
        }
        for (name, _) in THEME_CHOICES {
            assert!(theme::SEEDS.iter().any(|s| seed_value(s) == *name), "{name}");
        }
    }
}
