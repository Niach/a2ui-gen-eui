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

use std::collections::HashMap;

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

// The lexer matches greedily, so a string is one terminal for all four forms.
const LARK_TOKENS: &str = r#"
ident: IDENT
string: STRING
number: NUMBER
path: PATH
abspath: ABSPATH
IDENT: /[\p{XID_Start}_][\p{XID_Continue}]*/
STRING: /"""([^"\\]|\\[^\n]|"[^"\\]|""[^"\\])*"""|r"""([^"]|"[^"]|""[^"])*"""|r"[^"\n]*"|"([^"\\\n]|\\[^\n])*"/
NUMBER: /-?[0-9]+(\.[0-9]+)?([eE][-+]?[0-9]+)?/
PATH: /\$[A-Za-z0-9_\/]*/
ABSPATH: /\$(\/|(\/[A-Za-z0-9_]+)+)/
"#;

#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Gbnf,
    Lark,
}

/// The grammar for llama.cpp's GBNF sampler and compatible engines. The start rule is `root`.
pub fn gbnf(catalog: &Value) -> String {
    Builder::new(catalog, Dialect::Gbnf).build()
}

/// The grammar in llguidance's Lark dialect. The start rule is `start`.
pub fn lark(catalog: &Value) -> String {
    Builder::new(catalog, Dialect::Lark).build()
}

struct Builder<'a> {
    catalog: &'a Value,
    dialect: Dialect,
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
                "checks" => checkable = true,
                "style" => alts.push("\"style=\" anymap".into()),
                _ => alts.push(format!("{} {}", lit(&format!("{p}=")), self.conv(s))),
            }
        }
        if checkable {
            alts.push("\"checks=\" checks".into());
        }
        if alts.is_empty() {
            alts.push("\"visible=\" any".into());
        }
        let kw = self.rule(alts.join(" | "), "kw");
        format!("{} ({kw} (\",\" ws {kw})*)? \")\"", lit(&format!("{name}(")))
    }

    fn build(mut self) -> String {
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
        let mut out = format!(
            "{start} ::= {} (data \"\\n\")* \"root = \" component \"\\n\" (stmt \"\\n\")* \"</a2ui>\"\n\
             stmt ::= data | ident \" = \" component\n\
             data ::= abspath \" = \" dataval\n\
             component ::= {}\n\
             fname ::= {}\n",
            lit(crate::prompt::REPLY_START),
            comps.join(" | "),
            fnames.join(" | ")
        );
        out.push_str(SHARED.trim_start());
        for (name, body) in &self.rules {
            out.push_str(&format!("{name} ::= {body}\n"));
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
