//! A small GBNF recognizer, so the tests check the llama.cpp spelling of the grammar too: it reads the
//! subset [`a2ui_gen_eui::grammar::gbnf`] writes (literals, `[…]` classes, rule names, groups, `? * +` and `{m,n}`)
//! and tells whether the start rule `root` matches a whole text.

use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone)]
enum Node {
    Lit(Vec<char>),
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    Rule(String),
    Alt(Vec<Vec<Item>>),
}

#[derive(Debug, Clone)]
struct Item {
    node: Node,
    min: usize,
    max: Option<usize>,
}

pub struct Gbnf {
    rules: HashMap<String, Node>,
}

struct Reader<'a> {
    s: &'a [char],
    i: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.i += 1;
        }
    }
    fn escape(&mut self) -> char {
        let c = self.s[self.i];
        self.i += 1;
        let hex = |r: &mut Self, n: usize| {
            let h: String = r.s[r.i..r.i + n].iter().collect();
            r.i += n;
            char::from_u32(u32::from_str_radix(&h, 16).unwrap()).unwrap()
        };
        match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            'x' => hex(self, 2),
            'u' => hex(self, 4),
            other => other,
        }
    }
    fn char_in(&mut self) -> char {
        let c = self.s[self.i];
        self.i += 1;
        if c == '\\' { self.escape() } else { c }
    }
    fn alts(&mut self) -> Node {
        let mut alts = vec![self.seq()];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.seq());
        }
        Node::Alt(alts)
    }
    /// `{m}`, `{m,}` or `{m,n}`, read to its end.
    fn range(&mut self) -> (usize, Option<usize>) {
        let close = self.i + self.s[self.i..].iter().position(|&c| c == '}').unwrap();
        let body: String = self.s[self.i + 1..close].iter().collect();
        self.i = close + 1;
        let num = |t: &str| t.trim().parse::<usize>().unwrap();
        match body.split_once(',') {
            None => (num(&body), Some(num(&body))),
            Some((m, n)) if n.trim().is_empty() => (num(m), None),
            Some((m, n)) => (num(m), Some(num(n))),
        }
    }

    fn seq(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            let node = match self.peek() {
                None | Some('|' | ')') => return items,
                Some('"') => {
                    self.i += 1;
                    let mut text = Vec::new();
                    while self.peek() != Some('"') {
                        text.push(self.char_in());
                    }
                    self.i += 1;
                    Node::Lit(text)
                }
                Some('[') => {
                    self.i += 1;
                    let negated = self.peek() == Some('^');
                    if negated {
                        self.i += 1;
                    }
                    let mut ranges = Vec::new();
                    while self.peek() != Some(']') {
                        let a = self.char_in();
                        let b = if self.peek() == Some('-') && self.s.get(self.i + 1) != Some(&']') {
                            self.i += 1;
                            self.char_in()
                        } else {
                            a
                        };
                        ranges.push((a, b));
                    }
                    self.i += 1;
                    Node::Class { negated, ranges }
                }
                Some('(') => {
                    self.i += 1;
                    let n = self.alts();
                    assert_eq!(self.peek(), Some(')'));
                    self.i += 1;
                    n
                }
                Some(c) if c.is_ascii_alphanumeric() || c == '-' => {
                    let start = self.i;
                    while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '-') {
                        self.i += 1;
                    }
                    Node::Rule(self.s[start..self.i].iter().collect())
                }
                Some(c) => panic!("unexpected {c:?} in GBNF"),
            };
            let (min, max) = match self.peek() {
                Some('{') => self.range(),
                Some(c @ ('?' | '*' | '+')) => {
                    self.i += 1;
                    match c {
                        '?' => (0, Some(1)),
                        '*' => (0, None),
                        _ => (1, None),
                    }
                }
                _ => (1, Some(1)),
            };
            items.push(Item { node, min, max });
        }
    }
}

impl Gbnf {
    pub fn parse(text: &str) -> Gbnf {
        let mut rules = HashMap::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (name, body) = line
                .split_once(" ::= ")
                .unwrap_or_else(|| panic!("not a rule: {line}"));
            let chars: Vec<char> = body.chars().collect();
            let mut r = Reader { s: &chars, i: 0 };
            let node = r.alts();
            assert_eq!(r.i, chars.len(), "rule {name} not read to its end");
            assert!(rules.insert(name.to_owned(), node).is_none(), "rule {name} twice");
        }
        let g = Gbnf { rules };
        for node in g.rules.values() {
            g.check_refs(node);
        }
        g
    }

    fn check_refs(&self, node: &Node) {
        match node {
            Node::Rule(n) => assert!(self.rules.contains_key(n), "undefined rule {n}"),
            Node::Alt(alts) => alts.iter().flatten().for_each(|i| self.check_refs(&i.node)),
            _ => {}
        }
    }

    /// Whether `root` matches all of `text`.
    #[allow(dead_code)]
    pub fn accepts(&self, text: &str) -> bool {
        self.check(text).is_ok()
    }

    /// `Err` with the farthest character a terminal reached when `root` does not match all of `text`.
    #[allow(dead_code)]
    pub fn check(&self, text: &str) -> Result<(), usize> {
        let input: Vec<char> = text.chars().collect();
        let mut memo = HashMap::new();
        let ends = self.node(&Node::Rule("root".into()), &input, 0, &mut memo);
        if ends.contains(&input.len()) {
            return Ok(());
        }
        Err(memo.values().flatten().copied().max().unwrap_or(0))
    }

    fn node(
        &self,
        node: &Node,
        s: &[char],
        at: usize,
        memo: &mut HashMap<(String, usize), BTreeSet<usize>>,
    ) -> BTreeSet<usize> {
        match node {
            Node::Lit(text) => {
                let ok = s.len() >= at + text.len() && s[at..at + text.len()] == text[..];
                ok.then_some(at + text.len()).into_iter().collect()
            }
            Node::Class { negated, ranges } => match s.get(at) {
                Some(c) if ranges.iter().any(|(a, b)| a <= c && c <= b) != *negated => [at + 1].into(),
                _ => BTreeSet::new(),
            },
            Node::Rule(name) => {
                let key = (name.clone(), at);
                if let Some(ends) = memo.get(&key) {
                    return ends.clone();
                }
                memo.insert(key.clone(), BTreeSet::new()); // no left recursion in these grammars
                let ends = self.node(&self.rules[name], s, at, memo);
                memo.insert(key, ends.clone());
                ends
            }
            Node::Alt(alts) => alts.iter().flat_map(|seq| self.seq(seq, s, at, memo)).collect(),
        }
    }

    fn seq(
        &self,
        items: &[Item],
        s: &[char],
        at: usize,
        memo: &mut HashMap<(String, usize), BTreeSet<usize>>,
    ) -> BTreeSet<usize> {
        let mut here: BTreeSet<usize> = [at].into();
        for item in items {
            let mut next = BTreeSet::new();
            for &p in &here {
                // repeat the item: positions after k matches
                let mut frontier: BTreeSet<usize> = [p].into();
                let mut seen: BTreeSet<usize> = BTreeSet::new();
                let mut k = 0;
                loop {
                    if k >= item.min {
                        next.extend(frontier.iter().copied());
                    }
                    if item.max.is_some_and(|m| k >= m) || frontier.is_empty() {
                        break;
                    }
                    let mut grown = BTreeSet::new();
                    for &q in &frontier {
                        for e in self.node(&item.node, s, q, memo) {
                            // an empty match only counts where the item is matched once
                            let once = item.max == Some(1);
                            if (once || e > q) && seen.insert(e) {
                                grown.insert(e);
                            }
                        }
                    }
                    frontier = grown;
                    k += 1;
                }
            }
            here = next;
            if here.is_empty() {
                break;
            }
        }
        here
    }
}
