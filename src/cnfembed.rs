//! A standalone single-file conformant implementation of cnf.
//!
//! Translates cnf to json, preserving order and text style where
//! possible. This layout makes it optimal for copying and embedding
//! into projects that would like to use cnf for reading configuration
//! files but would rather not add an additional external dependency.

use std::borrow::Cow;
use std::fmt;
use std::fmt::Write as _;

/// Reserved for future parse and render settings.
pub struct Options;

/// One parse or render failure, at its source position.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Error {
    pub line: usize,
    pub column: usize,
    pub text: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.text)
    }
}

/// Translate cnf text to json text.
pub fn to_json(data: &str, _options: Option<&Options>) -> Result<String, Error> {
    let mut p = Parser::new(split_lines(data));
    let root = p.parse_top()?;
    if let Some(err) = p.first {
        return Err(err);
    }
    Ok(write_node_string(&p.arena, root))
}

//
// The tree: the whole document in map scopes
//

// One node: a key of one map scope and the value under it - a
// map, a list, or a scalar's pre-encoded json. A bare list element
// carries no key; a map's sub is never unset.
#[derive(Default)]
struct KeyNode<'src> {
    key: Cow<'src, str>, // the segment's decoded text; "" for a bare element
    quote: QuoteKind,    // the segment's quoting style, to spell in errors
    sub: Option<Vec<usize>>, // the map or list under this key
    list: bool,          // sub holds a list's elements, not a map's pairs
    json: String,        // the pre-encoded scalar value; "" = none
    expanded: bool,      // a map was written here already, once only
}

fn write_node_string(arena: &[KeyNode<'_>], root: usize) -> String {
    let mut b = String::new();
    write_node(arena, &mut b, root);
    b
}

// write_node writes the value face: a list's elements, a map's
// pairs, or the scalar json. Keys are the parent's to write - a
// map writes its pairs', a list writes no keys of its own.
fn write_node(arena: &[KeyNode<'_>], b: &mut String, n: usize) {
    let nd = &arena[n];
    if nd.list {
        b.push('[');
        for (i, &e) in nd.sub.as_deref().unwrap_or(&[]).iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            let ed = &arena[e];
            if !ed.key.is_empty() {
                // The element's key face: nothing below the list
                // writes keys, so the head's own key wraps here.
                b.push('{');
                write_json_string(b, &ed.key);
                b.push(':');
                write_node(arena, b, e);
                b.push('}');
                continue;
            }
            write_node(arena, b, e);
        }
        b.push(']');
    } else if let Some(sub) = &nd.sub {
        b.push('{');
        for (i, &e) in sub.iter().enumerate() {
            if i > 0 {
                b.push(',');
            }
            write_json_string(b, &arena[e].key);
            b.push(':');
            write_node(arena, b, e);
        }
        b.push('}');
    } else {
        b.push_str(&nd.json);
    }
}

//
// The source positions
//

// A source position for errors: 1-based line and column.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Pos {
    l: usize,
    c: usize,
}

fn err_at(p: Pos, msg: impl Into<String>) -> Error {
    Error { line: p.l, column: p.c, text: msg.into() }
}

// A scanner error: the offset names where the offender sits
// within the scanned text, or no offset when the whole text is
// already the position.
enum ScanError {
    At { offset: usize, msg: String },
    Plain(String),
}

impl ScanError {
    fn at(offset: usize, msg: impl Into<String>) -> Self {
        ScanError::At { offset, msg: msg.into() }
    }
    fn plain(msg: impl Into<String>) -> Self {
        ScanError::Plain(msg.into())
    }
    // Move an offset into the whole path's frame.
    fn shift(self, by: usize) -> Self {
        match self {
            ScanError::At { offset, msg } => ScanError::At { offset: offset + by, msg },
            other => other,
        }
    }
    fn message(&self) -> &str {
        match self {
            ScanError::At { msg, .. } => msg,
            ScanError::Plain(msg) => msg,
        }
    }
}

// Wrap a scan error at the source position where the scanned text
// begins: pos advanced over the offset the error names.
fn err_at_scan(p: Pos, text: &str, err: ScanError) -> Error {
    match err {
        ScanError::Plain(msg) => err_at(p, msg),
        ScanError::At { offset, msg } => {
            let offset = offset.min(text.len());
            let delta = &text[..offset];
            let l = p.l + delta.bytes().filter(|&b| b == b'\n').count();
            let c = match delta.rfind('\n') {
                Some(last) => delta[last + 1..].chars().count() + 1,
                None => p.c + delta.chars().count(),
            };
            err_at(Pos { l, c }, msg)
        }
    }
}

//
// The document lines
//

// One physical line of the document, split into its indentation
// prefix and its content.
#[derive(Clone, Copy)]
struct Line<'src> {
    num: usize,        // 1-based line number, for errors
    indent: &'src str, // the literal prefix, spaces or tabs
    text: &'src str,   // content after the prefix
}

fn split_lines<'src>(data: &'src str) -> Vec<Line<'src>> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut raws: Vec<&str> = data.split('\n').collect();
    if data.ends_with('\n') {
        raws.pop(); // the final newline ends the last line
    }
    raws
        .iter()
        .enumerate()
        .map(|(n, raw)| {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let cut = raw.bytes().take_while(|&b| b == b' ' || b == b'\t').count();
            Line { num: n + 1, indent: &raw[..cut], text: &raw[cut..] }
        })
        .collect()
}

fn indent_pos(ln: &Line<'_>) -> Pos {
    Pos { l: ln.num, c: ln.indent.len() + 1 }
}

// quote_short quotes a short prefix of s for an error message:
// up to five runes as they stand, then "..." when more follow.
fn quote_short(s: &str) -> String {
    let cut = if s.len() > 5 {
        format!("{}...", String::from_utf8_lossy(&s.as_bytes()[..5]))
    } else {
        s.to_string()
    };
    format!("`{}`", escape(&cut))
}

/// Spell s by the double-quote rule, without the quotes.
pub fn escape(s: &str) -> String {
    let mut out = String::new();
    for r in s.chars() {
        out.push_str(&escape_cnf(r));
    }
    out
}

//
// The parser: lines read into map scopes
//

// One key path's segment, fresh from the scanner. The offset is
// the segment's within its scanned path; pos is assigned by the
// caller who knows the line.
#[derive(Clone)]
struct Seg<'src> {
    key: Cow<'src, str>,
    quote: QuoteKind,
    pos: Pos,
    offset: usize,
}

struct Parser<'src> {
    arena: Vec<KeyNode<'src>>,
    lines: Vec<Line<'src>>,
    pos: usize,
    pending: Vec<Line<'src>>, // comment and blank lines, not yet validated
    first: Option<Error>,      // the first key conflict, raised after the whole parse
}

impl<'src> Parser<'src> {
    fn new(lines: Vec<Line<'src>>) -> Self {
        Parser { arena: vec![KeyNode::default()], lines, pos: 0, pending: Vec::new(), first: None }
    }

    fn save_err(&mut self, err: Error) {
        if self.first.is_none() {
            self.first = Some(err);
        }
    }

    // A fresh map node under no key yet.
    fn new_map(&mut self) -> usize {
        self.push(KeyNode { sub: Some(Vec::new()), ..Default::default() })
    }

    // A scalar node holding pre-encoded json.
    fn scalar(&mut self, json: String) -> usize {
        self.push(KeyNode { json, ..Default::default() })
    }

    fn push(&mut self, n: KeyNode<'src>) -> usize {
        self.arena.push(n);
        self.arena.len() - 1
    }

    // find returns the child of scope holding key, if any.
    fn find(&self, scope: usize, key: &str) -> Option<usize> {
        let sub = self.arena[scope].sub.as_ref()?;
        sub.iter().copied().find(|&e| self.arena[e].key == key)
    }

    // walk descends the path's inner segments, creating
    // pass-through maps. Only a definition - a scalar or a list -
    // blocks a path; a map is traversed, however spelled.
    fn walk(&mut self, scope: usize, path: &[Seg<'src>]) -> Result<usize, Error> {
        let mut scope = scope;
        for seg in &path[..path.len() - 1] {
            let node = match self.find(scope, &seg.key) {
                Some(n) => n,
                None => {
                    let n = self.push(KeyNode {
                        key: seg.key.clone(),
                        quote: seg.quote,
                        sub: Some(Vec::new()),
                        ..Default::default()
                    });
                    self.arena[scope].sub.get_or_insert_with(Vec::new).push(n);
                    n
                }
            };
            if !self.arena[node].json.is_empty() || self.arena[node].list {
                let qk = quote(&self.arena[node].key, self.arena[node].quote, true);
                return Err(err_at(
                    seg.pos,
                    format!("cannot redefine previous key {qk}"),
                ));
            }
            scope = node;
        }
        Ok(scope)
    }

    // define places a non-map value: the tip must carry nothing
    // yet. A redefinition reports the whole path, anchored at its
    // first segment.
    fn define(&mut self, scope: usize, path: &[Seg<'src>], val: usize) -> Result<(), Error> {
        let parent = self.walk(scope, path)?;
        let tip = path.last().unwrap();
        if self.find(parent, &tip.key).is_some() {
            return Err(redefine_path(tip));
        }
        let v = std::mem::take(&mut self.arena[val]);
        let n = self.push(KeyNode {
            key: tip.key.clone(),
            quote: tip.quote,
            json: v.json,
            list: v.list,
            sub: v.sub,
            ..Default::default()
        });
        self.arena[parent].sub.get_or_insert_with(Vec::new).push(n);
        Ok(())
    }

    // expand opens the tip's map for its items to be read
    // straight into, once only: a map already written there is a
    // second expansion.
    fn expand(&mut self, scope: usize, path: &[Seg<'src>]) -> Result<usize, Error> {
        let parent = self.walk(scope, path)?;
        let tip = path.last().unwrap();
        let node = match self.find(parent, &tip.key) {
            Some(n) => n,
            None => {
                let n = self.push(KeyNode {
                    key: tip.key.clone(),
                    quote: tip.quote,
                    sub: Some(Vec::new()),
                    ..Default::default()
                });
                self.arena[parent].sub.get_or_insert_with(Vec::new).push(n);
                n
            }
        };
        if !self.arena[node].json.is_empty() || self.arena[node].list {
            return Err(redefine_path(tip));
        }
        if self.arena[node].expanded {
            return Err(expand_path_err(tip));
        }
        self.arena[node].expanded = true;
        Ok(node)
    }

    // wrap_path wraps a keyed element's path around its value:
    // one single-pair level per segment, the tip holding it.
    fn wrap_path(&mut self, path: &[Seg<'src>], val: usize) -> usize {
        let tip = path.last().unwrap();
        let v = std::mem::take(&mut self.arena[val]);
        let mut n = self.push(KeyNode {
            key: tip.key.clone(),
            quote: tip.quote,
            json: v.json,
            list: v.list,
            sub: v.sub,
            ..Default::default()
        });
        for seg in path[..path.len() - 1].iter().rev() {
            n = self.push(KeyNode {
                key: seg.key.clone(),
                quote: seg.quote,
                sub: Some(vec![n]),
                ..Default::default()
            });
        }
        n
    }

    // peek_struct returns the next structural line, collecting
    // the blank and comment lines it skips into the pending run.
    // A shallower comment closes the block: returned unconsumed.
    fn peek_struct(&mut self, prefix: &'src str) -> Option<Line<'src>> {
        while self.pos < self.lines.len() {
            let ln = &self.lines[self.pos];
            if ln.text.is_empty() {
                self.pending.push(*ln);
                self.pos += 1;
                continue;
            }
            if ln.text.starts_with('#') {
                if ln.indent.len() < prefix.len() {
                    return Some(*ln);
                }
                self.pending.push(*ln);
                self.pos += 1;
                continue;
            }
            return Some(*ln);
        }
        None
    }

    // check_pending validates the pending run at the block's exact
    // prefix and drops it: json cannot carry comments, so only the
    // indent rule stands.
    fn check_pending(&mut self, prefix: &str) -> Result<(), Error> {
        let err = cmt_indent_ok(&self.pending, prefix);
        self.pending.clear();
        err
    }

    // Nothing may sit between the pair and the fence that opens
    // under it: neither a comment nor a blank line.
    fn cmt_no_fence_prefix(&mut self) -> Result<(), Error> {
        if let Some(ln) = self.pending.first() {
            if !ln.text.is_empty() {
                return Err(err_at(indent_pos(ln), "cannot comment between key and fence"));
            }
            return Err(err_at(indent_pos(ln), "cannot blank between key and fence"));
        }
        Ok(())
    }

    // parse_top reads the file's one value into the root node.
    fn parse_top(&mut self) -> Result<usize, Error> {
        let first = self.peek_struct("");
        self.check_pending("")?;
        let Some(first) = first else {
            // An empty file is an empty map.
            return Ok(self.new_map());
        };
        if looks_like_item(&first.text) {
            // A pair-shaped line opens a map.
            let root = self.new_map();
            self.parse_map(root, "")?;
            return Ok(root);
        }
        // Any other value opens on its own line, at column zero.
        if !first.indent.is_empty() {
            return Err(err_at(indent_pos(&first), "cannot indent a top-level value"));
        }
        let root = self.parse_block_value(first)?;
        if let Some(ln) = self.peek_struct("") {
            // Stray content after the top-level value.
            return Err(err_at(
                indent_pos(&ln),
                format!("cannot parse content after top-level value: {}", quote_short(&ln.text)),
            ));
        }
        Ok(root)
    }

    // parse_map reads one block map's pairs at the exact prefix
    // into the scope.
    fn parse_map(&mut self, scope: usize, parent_indent: &'src str) -> Result<(), Error> {
        loop {
            let Some(ln) = self.peek_struct(parent_indent) else {
                return self.check_pending(parent_indent);
            };
            if ln.indent != parent_indent {
                if ln.indent.len() > parent_indent.len() {
                    // The deeper line's own start: the pair that
                    // misses the block's prefix is named there.
                    return Err(err_at(indent_pos(&ln), "cannot mix indentation prefixes"));
                }
                // A shallower line - structural or comment - closes
                // the block; it stays in the stream for the parent.
                return self.check_pending(parent_indent);
            }
            self.check_pending(parent_indent)?;
            self.pos += 1;
            self.parse_item(ln, scope)?;
        }
    }

    // parse_item reads one "path: value" line into the scope.
    // A token at offset i in the line's text sits at column
    // len(indent)+1+i.
    fn parse_item(&mut self, ln: Line<'src>, scope: usize) -> Result<(), Error> {
        let base = ln.indent.len() + 1;
        let (mut path, after) = match scan_key_path(&ln.text) {
            Ok(v) => v,
            Err(e) => return Err(err_at_scan(Pos { l: ln.num, c: base }, &ln.text, e)),
        };
        if after.is_empty() || !after.starts_with(':') {
            let c = base + ln.text.len() - after.len();
            return Err(err_at(Pos { l: ln.num, c }, "cannot find key-value separator `:`"));
        }
        for seg in &mut path {
            seg.pos = Pos { l: ln.num, c: base + seg.offset };
        }
        let text = after[1..].trim_start_matches([' ', '\t']);
        if !text.is_empty() {
            let rest = self.scan_pair_into(&ln, scope, &path, text)?;
            return trailing(&ln, rest);
        }
        let next = self.peek_struct(ln.indent);
        if let Some(nx) = &next {
            if !ln.indent.is_empty() && !same_indent_char(&nx.indent, ln.indent.as_bytes()[0]) {
                return Err(err_at(indent_pos(nx), "cannot mix indentation prefixes"));
            }
        }
        let Some(next) = next else {
            return Err(err_at(
                indent_pos(&ln),
                format!("cannot omit the value for {}", quote_key_path(&path)),
            ));
        };
        if next.indent.len() <= ln.indent.len() {
            return Err(err_at(
                indent_pos(&ln),
                format!("cannot omit the value for {}", quote_key_path(&path)),
            ));
        }
        if is_list_item(&next.text) {
            // A dashed block: a value defined under the path like
            // any other.
        } else if is_string_fence(&next.text) || next.text == "---" {
            // Nothing may sit between the pair and its fence.
            self.cmt_no_fence_prefix()?;
        } else {
            // A block map: the path expands once, and its items
            // read straight into the expansion. A conflict is
            // kept, not raised: the items still parse, into a
            // scope the error path drops.
            let tip = match self.expand(scope, &path) {
                Ok(t) => t,
                Err(e) => {
                    self.save_err(e);
                    self.new_map()
                }
            };
            return self.parse_map(tip, next.indent);
        }
        let val = self.parse_block_value(next)?;
        if let Err(e) = self.define(scope, &path, val) {
            self.save_err(e);
        }
        Ok(())
    }

    // scan_pair_into reads a pair's inline value: an inline map
    // expands the path, anything else is defined under it.
    fn scan_pair_into(
        &mut self,
        ln: &Line<'src>,
        scope: usize,
        path: &[Seg<'src>],
        text: &'src str,
    ) -> Result<&'src str, Error> {
        if !text.is_empty() && text.as_bytes()[0] == b'{' {
            let tip = match self.expand(scope, path) {
                Ok(t) => t,
                Err(e) => {
                    self.save_err(e);
                    self.new_map()
                }
            };
            let pt = Pos { l: ln.num, c: ln.text.len() - text.len() + ln.indent.len() + 1 };
            return self.scan_inline_map_into(ln, text, pt, tip);
        }
        let (val, rest) = self.scan_value_at(ln, text)?;
        if let Err(e) = self.define(scope, path, val) {
            self.save_err(e);
        }
        Ok(rest)
    }

    // parse_block_list reads one block list's "- item" lines at
    // the exact prefix, keyed elements' paths wrapped around
    // their values.
    fn parse_block_list(&mut self, prefix: &'src str) -> Result<usize, Error> {
        let mut elems = Vec::new();
        loop {
            let ln = self.peek_struct(prefix);
            let opens_item = ln
                .as_ref()
                .map_or(false, |l| l.indent == prefix && is_list_item(&l.text));
            if !opens_item {
                self.check_pending(prefix)?;
                return Ok(self.push(KeyNode {
                    list: true,
                    sub: Some(elems),
                    ..Default::default()
                }));
            }
            let ln = ln.unwrap();
            self.check_pending(prefix)?;
            self.pos += 1;
            let mut text = ln.text[1..].trim_start_matches([' ', '\t']);
            if text.is_empty() {
                // A dash with nothing after it.
                return Err(err_at(indent_pos(&ln), "cannot omit the value"));
            }
            let mut path: Option<Vec<Seg<'src>>> = None;
            // The inlinepair form: a path and a colon before the
            // value; without them the whole text is the value.
            if let Ok((mut k, rest)) = scan_key_path(text) {
                if !rest.is_empty() && rest.starts_with(':') {
                    let off = ln.text.len() - text.len();
                    for seg in &mut k {
                        seg.pos = Pos { l: ln.num, c: ln.indent.len() + 1 + off + seg.offset };
                    }
                    path = Some(k);
                    text = rest[1..].trim_start_matches([' ', '\t']);
                    if text.is_empty() {
                        // A path ending on its colon: the same
                        // omission parse_item reports.
                        return Err(err_at(
                            indent_pos(&ln),
                            format!(
                                "cannot omit the value for {}",
                                quote_key_path(path.as_ref().unwrap())
                            ),
                        ));
                    }
                }
            }
            let mut val = self.parse_inline(&ln, text)?;
            if let Some(p) = &path {
                val = self.wrap_path(p, val);
            }
            elems.push(val);
        }
    }

    // parse_fence reads one fence block from its opening line,
    // already peeked as next, into its json string. In the
    // escaped family a trailing backslash continues the line.
    fn parse_fence(&mut self, next: Line<'src>, delim: &str) -> Result<String, Error> {
        let prefix = next.indent;
        self.pos += 1; // consume the opening delimiter
        let escaped = delim == r#"""""#;
        let mut b = String::from("\"");
        let mut indent_char = b' ';
        if !prefix.is_empty() {
            indent_char = prefix.as_bytes()[0];
        }
        let mut continued = false;
        while self.pos < self.lines.len() {
            let ln = self.lines[self.pos];
            self.pos += 1;
            // The close wins even over a continuation: the escape
            // ate the newline, the value ends without one.
            if ln.indent == prefix && ln.text == delim {
                b.push('"');
                return Ok(b);
            }
            if !continued && !same_indent_char(&ln.indent, indent_char) {
                return Err(err_at(indent_pos(&ln), "cannot mix indentation prefixes in fence"));
            }
            if ln.indent.len() < prefix.len() {
                if !ln.text.is_empty() {
                    // Only the exact prefix closes the fence.
                    return Err(err_at(indent_pos(&ln), "cannot close fence at shallower prefix"));
                }
                // A blank line with a valid but shallower prefix:
                // the line contributes the newline only.
                b.push_str("\\n");
                continue;
            }
            if ln.text.is_empty() {
                // A blank line at or deeper than the fence prefix:
                // its whitespace is content, with its terminator.
                write_escaped_json(&mut b, &ln.indent[prefix.len()..]);
                b.push_str("\\n");
                continue;
            }
            if !ln.indent.starts_with(&prefix) {
                return Err(err_at(indent_pos(&ln), "cannot mix indentation prefixes in fence"));
            }
            // The content following the valid prefix is the string,
            // including any excess indentation on deeper lines.
            let mut content = format!("{}{}", &ln.indent[prefix.len()..], ln.text);
            let content_pos = Pos { l: ln.num, c: prefix.len() + 1 };
            continued = false;
            if escaped && content.contains('\\') {
                // Only a backslash can unescape or continue a line;
                // plain content flows to the reescaper as it stands.
                if odd_trailing_backslashes(&content) {
                    content.pop();
                    continued = true;
                }
                if let Err(e) = write_reescaped_json(&mut b, &content) {
                    return Err(err_at_scan(content_pos, &content, e));
                }
                if !continued {
                    b.push_str("\\n");
                }
                continue;
            }
            write_escaped_json(&mut b, &content);
            if !continued {
                b.push_str("\\n");
            }
        }
        Err(err_at(indent_pos(&next), "cannot find closing fence"))
    }

    // parse_list_fence reads one --- delimited list from its
    // opening line, each element the lines between two --- at the
    // fence's exact prefix, parsed as a block value.
    fn parse_list_fence(&mut self, next: Line<'src>) -> Result<usize, Error> {
        let prefix = next.indent;
        self.pos += 1; // consume the opening ---
        let mut elems = Vec::new();
        let mut nested: Option<&'src str> = None; // the nested fences' indent, once set
        loop {
            let start = self.pos;
            let mut closed = false;
            while self.pos < self.lines.len() {
                let cur = &self.lines[self.pos];
                if cur.indent == prefix && cur.text == "---" {
                    closed = true;
                    break;
                }
                self.pos += 1;
            }
            if !closed {
                // The fence that never closes is named where it
                // opened: its own position.
                return Err(err_at(indent_pos(&next), "cannot find closing list fence"));
            }
            let elem = &self.lines[start..self.pos];
            if elem.is_empty() {
                let open = if start > 0 { &self.lines[start - 1] } else { &next };
                return Err(err_at(indent_pos(open), "cannot parse empty list fence element"));
            }
            // Nested fences must share one indent: the first sets
            // it, a mismatched one mixes indents inside the fence.
            if elem[0].text == "---" && elem[0].indent.len() > prefix.len() {
                match nested {
                    None => nested = Some(elem[0].indent),
                    Some(prev) if prev != elem[0].indent => {
                        return Err(err_at(
                            indent_pos(&elem[0]),
                            "cannot mix indents inside list fence",
                        ));
                    }
                    _ => {}
                }
            }
            self.pos += 1; // consume the closing ---
            // The element parses as its own document, into this
            // same arena: lent to the sub-parser, then taken back.
            let arena = std::mem::take(&mut self.arena);
            let mut sub =
                Parser { arena, lines: elem.to_vec(), pos: 0, pending: Vec::new(), first: None };
            let parsed = (|sub: &mut Parser<'src>| -> Result<usize, Error> {
                let Some(first) = sub.peek_struct(prefix) else {
                    return Err(err_at(indent_pos(&next), "cannot parse empty list fence element"));
                };
                // The fence's prefix delimits the element: a deeper
                // line opens a nested block only as a list fence.
                if first.indent != prefix
                    && !(first.text == "---" && first.indent.len() > prefix.len())
                {
                    return Err(err_at(indent_pos(&first), "list element indent must align with fence"));
                }
                let val = sub.parse_block_value(first)?;
                if sub.pos != sub.lines.len() {
                    // Content after the element's value.
                    return Err(err_at(
                        indent_pos(&sub.lines[sub.pos]),
                        "cannot parse content in list fence element",
                    ));
                }
                Ok(val)
            })(&mut sub);
            let sub_first = sub.first.take();
            self.arena = sub.arena; // take the arena back
            if let Some(e) = sub_first {
                self.save_err(e); // the element's own conflicts count too
            }
            let val = parsed?;
            elems.push(val);
            // The fence continues while the next line sits at its
            // prefix or deeper; a shallower line closes it and
            // stays in the stream for the parent.
            match self.peek_struct(prefix) {
                None => {
                    return Ok(self.push(KeyNode {
                        list: true,
                        sub: Some(elems),
                        ..Default::default()
                    }))
                }
                Some(ln) if ln.indent.len() < prefix.len() => {
                    return Ok(self.push(KeyNode {
                        list: true,
                        sub: Some(elems),
                        ..Default::default()
                    }))
                }
                _ => {}
            }
        }
    }

    // parse_block_value reads the block value opening at first,
    // a peeked line.
    fn parse_block_value(&mut self, first: Line<'src>) -> Result<usize, Error> {
        if is_string_fence(first.text) {
            let s = self.parse_fence(first, first.text)?;
            Ok(self.scalar(s))
        } else if first.text == "---" {
            self.parse_list_fence(first)
        } else if is_list_item(first.text) {
            self.parse_block_list(first.indent)
        } else if looks_like_item(first.text) {
            let m = self.new_map();
            self.parse_map(m, first.indent)?;
            Ok(m)
        } else {
            // An inline value on its only line.
            self.pos += 1; // consume the value's own line
            self.parse_inline(&first, first.text)
        }
    }

    // parse_inline reads one inline value, then a trailing
    // comment, on the item's one line.
    fn parse_inline(&mut self, ln: &Line<'src>, text: &'src str) -> Result<usize, Error> {
        let (val, rest) = self.scan_value_at(ln, text)?;
        trailing(ln, rest)?;
        Ok(val)
    }

    // scan_value_at reads one inline value from the front of text,
    // returning the unread remainder.
    fn scan_value_at(&mut self, ln: &Line<'src>, text: &'src str) -> Result<(usize, &'src str), Error> {
        let pt = Pos { l: ln.num, c: ln.text.len() - text.len() + ln.indent.len() + 1 };
        if text.is_empty() {
            // An empty scan: the usual source is an unclosed
            // list's spent tail.
            return Err(err_at(pt, r#"cannot parse value at """#));
        }
        match text.as_bytes()[0] {
            b'{' => {
                // A fresh scope: nothing later reaches into a
                // value-position map.
                let m = self.new_map();
                let rest = self.scan_inline_map_into(ln, text, pt, m)?;
                Ok((m, rest))
            }
            b'[' => self.scan_inline_list(ln, text, pt),
            b'"' => {
                if text.starts_with(r#"""""#) {
                    return self.scan_fenced_line(ln, text, b'"', pt);
                }
                let (s, rest) =
                    scan_double_quoted(text).map_err(|e| err_at_scan(pt, text, e))?;
                Ok((self.scalar(json_string(&s)), rest))
            }
            b'\'' => {
                if text.starts_with("'''") {
                    return self.scan_fenced_line(ln, text, b'\'', pt);
                }
                let (s, rest) =
                    scan_single_quoted(text).map_err(|e| err_at(pt, e.message()))?;
                Ok((self.scalar(json_string(&s)), rest))
            }
            _ => {
                let (tok, rest) = scan_scalar_token(text);
                match tok {
                    "null" | "true" | "false" => Ok((self.scalar(tok.to_string()), rest)),
                    "" => Err(err_at(pt, format!("cannot parse value at `{}`", escape(text)))),
                    _ => {
                        let json = scan_number(tok, pt)?;
                        Ok((self.scalar(json), rest))
                    }
                }
            }
        }
    }

    // scan_inline_list reads "[1, 2]" in the one-line form.
    fn scan_inline_list(&mut self, ln: &Line<'src>, text: &'src str, at: Pos) -> Result<(usize, &'src str), Error> {
        let mut elems = Vec::new();
        let mut text = text[1..].trim_start_matches([' ', '\t']);
        if text.starts_with(']') {
            return Ok((self.push(KeyNode { list: true, ..Default::default() }), &text[1..]));
        }
        loop {
            let (val, rest) = self.scan_value_at(ln, text)?;
            elems.push(val);
            let rest = rest.trim_start_matches([' ', '\t']);
            if rest.is_empty() {
                // Named where it opened, so nested lists tell apart.
                return Err(err_at(at, "cannot find closing `]`"));
            }
            if rest.starts_with(']') {
                return Ok((self.push(KeyNode {
                    list: true,
                    sub: Some(elems),
                    ..Default::default()
                }), &rest[1..]));
            }
            if !rest.starts_with(',') {
                let c = ln.indent.len() + 1 + ln.text.len() - rest.len();
                return Err(err_at(
                    Pos { l: ln.num, c },
                    format!("cannot parse `{}` inside list", escape(rest)),
                ));
            }
            text = rest[1..].trim_start_matches([' ', '\t']);
        }
    }

    // scan_inline_map_into reads "{a: 1, b: 2}" in the native item
    // grammar into the scope, returning the unread remainder.
    fn scan_inline_map_into(&mut self, ln: &Line<'src>, text: &'src str, at: Pos, scope: usize) -> Result<&'src str, Error> {
        let mut text = text[1..].trim_start_matches([' ', '\t']);
        if text.starts_with('}') {
            return Ok(&text[1..]);
        }
        loop {
            let off = ln.text.len() - text.len();
            let (mut path, after) = match scan_key_path(text) {
                Ok(v) => v,
                Err(e) => return Err(err_at_scan(Pos { l: ln.num, c: ln.indent.len() + 1 + off }, text, e)),
            };
            if after.is_empty() || !after.starts_with(':') {
                let c = ln.indent.len() + 1 + off + text.len() - after.len();
                return Err(err_at(Pos { l: ln.num, c }, "cannot find key-value separator `:`"));
            }
            for seg in &mut path {
                seg.pos = Pos { l: ln.num, c: ln.indent.len() + 1 + off + seg.offset };
            }
            let rest = self.scan_pair_into(ln, scope, &path, after[1..].trim_start_matches([' ', '\t']))?;
            let rest = rest.trim_start_matches([' ', '\t']);
            if rest.is_empty() {
                // Named where it opened, so nested maps tell apart.
                return Err(err_at(at, "cannot find closing `}`"));
            }
            if rest.starts_with('}') {
                return Ok(&rest[1..]);
            }
            if !rest.starts_with(',') {
                let c = ln.indent.len() + 1 + ln.text.len() - rest.len();
                return Err(err_at(
                    Pos { l: ln.num, c },
                    format!("cannot parse `{}` inside map", escape(rest)),
                ));
            }
            text = rest[1..].trim_start_matches([' ', '\t']);
        }
    }

    // scan_fenced_line reads a one-line fence. The first delimiter
    // closes; in the escaped family the search skips escapes.
    fn scan_fenced_line(&mut self, _ln: &Line<'src>, text: &'src str, quote_ch: u8, at: Pos) -> Result<(usize, &'src str), Error> {
        let delim: &str = if quote_ch == b'"' { r#"""""# } else { "'''" };
        let end: Option<usize> = if quote_ch == b'"' {
            let bytes = text.as_bytes();
            let mut i = 3;
            while i + 3 <= bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2; // skip the escape; its quote is content
                    continue;
                }
                if bytes[i..].starts_with(delim.as_bytes()) {
                    break;
                }
                i += 1;
            }
            if i + 3 <= bytes.len() && bytes[i..].starts_with(delim.as_bytes()) {
                Some(i - 3)
            } else {
                None
            }
        } else {
            text[3..].find(delim)
        };
        let Some(end) = end else {
            return Err(err_at(at, "cannot find closing fence"));
        };
        let inner = &text[3..3 + end];
        let inner_pos = Pos { l: at.l, c: at.c + 3 };
        let rest = &text[3 + end + 3..];
        if !needs_escape(inner) {
            return Ok((self.scalar(format!("\"{inner}\"")), rest));
        }
        if quote_ch == b'"' {
            let mut b = String::with_capacity(inner.len() + 2);
            b.push('"');
            if let Err(e) = write_reescaped_json(&mut b, inner) {
                return Err(err_at_scan(inner_pos, inner, e));
            }
            b.push('"');
            return Ok((self.scalar(b), rest));
        }
        // The raw family holds the interior as it stands.
        Ok((self.scalar(json_string(inner)), rest))
    }
}

// trailing accepts what follows a value on its line: nothing,
// or a comment.
fn trailing(ln: &Line<'_>, rest: &str) -> Result<(), Error> {
    let (left, ok) = strip_trail(rest);
    if ok || left.is_empty() {
        return Ok(());
    }
    Err(err_at(
        Pos { l: ln.num, c: ln.indent.len() + 1 + ln.text.len() - left.len() },
        format!("cannot parse trailing `{}`", escape(left)),
    ))
}

// strip_trail splits a trailing comment - at least one blank,
// then # - off the rest of a value. A # glued to the value is
// content, not a comment.
fn strip_trail(rest: &str) -> (&str, bool) {
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b'#' {
        return (rest, false);
    }
    ("", true)
}

fn cmt_indent_ok(run: &[Line<'_>], parent: &str) -> Result<(), Error> {
    for ln in run {
        if !ln.text.is_empty() && ln.indent != parent {
            return Err(err_at(indent_pos(ln), "cannot mix indentation prefixes"));
        }
    }
    Ok(())
}

// same_indent_char reports whether the prefix consists solely of
// the given indent character.
fn same_indent_char(prefix: &str, c: u8) -> bool {
    prefix.bytes().all(|b| b == c)
}

// odd_trailing_backslashes reports whether the text ends on an
// odd run of backslashes - an escape that ate the newline.
fn odd_trailing_backslashes(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut n = 0;
    while n < bytes.len() && bytes[bytes.len() - 1 - n] == b'\\' {
        n += 1;
    }
    n % 2 == 1
}

// looks_like_item is a cheap check for a "path: value" line:
// the real scan names the path, not the value, on a bad line.
fn looks_like_item(text: &str) -> bool {
    let Some(i) = text.find(':') else { return false };
    if i == 0 {
        return false;
    }
    let c = text.as_bytes()[0];
    c == b'"' || c == b'\'' || c == b'.' || is_bare_rune(c as char)
}

// is_list_item reports whether text opens a block list item.
fn is_list_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ") || text.starts_with("-\t")
}

// is_string_fence reports whether text is a fence delimiter.
fn is_string_fence(text: &str) -> bool {
    text == r#"""""# || text == "'''"
}

//
// The byte scanners: the grammar's tokens, errors carrying the
// offender's offset.
//

// scan_key_path reads a dotted path, recording each segment's
// offset within the original text for position tracking. Spaces
// never separate path parts.
fn scan_key_path<'src>(text: &'src str) -> Result<(Vec<Seg<'src>>, &'src str), ScanError> {
    if text.is_empty() || text.as_bytes()[0] == b'.' {
        return Err(ScanError::at(0, "cannot parse empty key segment"));
    }
    let mut segs = Vec::new();
    let mut text = text;
    let mut consumed = 0;
    loop {
        let (mut seg, after) = scan_key(text).map_err(|e| e.shift(consumed))?;
        seg.offset = consumed;
        segs.push(seg);
        consumed += text.len() - after.len();
        if !after.is_empty() && after.as_bytes()[0] == b'.' {
            if after.len() == 1 {
                return Err(ScanError::at(consumed + 1, "cannot parse empty key segment"));
            }
            if after.as_bytes()[1] == b' ' || after.as_bytes()[1] == b'\t' {
                return Err(ScanError::at(consumed + 1, "cannot parse space in path"));
            }
            text = &after[1..];
            consumed += 1;
            continue;
        }
        if !after.is_empty() && (after.as_bytes()[0] == b' ' || after.as_bytes()[0] == b'\t') {
            return Err(ScanError::at(consumed, "cannot parse space in path"));
        }
        return Ok((segs, after));
    }
}

// scan_key reads one key segment: a bare word or a quoted string.
fn scan_key<'src>(text: &'src str) -> Result<(Seg<'src>, &'src str), ScanError> {
    if text.is_empty() {
        return Err(ScanError::at(0, "cannot parse empty key segment"));
    }
    match text.as_bytes()[0] {
        b'"' => {
            let (s, rest) = scan_double_quoted(text)?;
            Ok((seg_new(s, QuoteKind::DoubleQuoted), rest))
        }
        b'\'' => {
            let (s, rest) = scan_single_quoted(text)?;
            Ok((seg_new(s, QuoteKind::SingleQuoted), rest))
        }
        _ => {
            let (word, rest) = scan_word(text);
            if word.is_empty() {
                return Err(ScanError::at(0, "cannot parse empty key segment"));
            }
            for r in word.chars() {
                if !is_bare_rune(r) {
                    // Named at the segment start, so the path's
                    // consumed count locates it.
                    return Err(ScanError::at(0, format!("cannot parse key `{}`", escape(word))));
                }
            }
            Ok((seg_new(Cow::Borrowed(word), QuoteKind::Unquoted), rest))
        }
    }
}

fn seg_new<'src>(key: Cow<'src, str>, quote: QuoteKind) -> Seg<'src> {
    Seg { key, quote, pos: Pos::default(), offset: 0 }
}

// scan_double_quoted reads a double-quoted string, processing
// escapes. The common escape-free string returns borrowed.
fn scan_double_quoted<'src>(text: &'src str) -> Result<(Cow<'src, str>, &'src str), ScanError> {
    let bytes = text.as_bytes();
    if let Some(i) = text[1..].find(['"', '\\']) {
        if bytes[1 + i] != b'\\' {
            return Ok((Cow::Borrowed(&text[1..1 + i]), &text[2 + i..]));
        }
    }
    let mut b = String::new();
    let mut i = 1;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            return Ok((Cow::Owned(b), &text[i + 1..]));
        }
        if c == b'\\' {
            if i + 1 >= bytes.len() {
                return Err(ScanError::at(i, "cannot end string on an escape"));
            }
            let (r, n) = scan_escape(&text[i..]).map_err(|e| ScanError::at(i, e))?;
            b.push(r);
            i += n;
            continue;
        }
        // A multi-byte rune flows through as its whole bytes.
        let start = i;
        while i < bytes.len() && bytes[i] != b'"' && bytes[i] != b'\\' {
            i += 1;
        }
        b.push_str(&text[start..i]);
    }
    Err(ScanError::plain("cannot find closing quote"))
}

// scan_single_quoted reads a single-quoted string: raw, no escapes.
fn scan_single_quoted<'src>(text: &'src str) -> Result<(Cow<'src, str>, &'src str), ScanError> {
    let Some(end) = text[1..].find('\'') else {
        return Err(ScanError::plain("cannot find closing quote"));
    };
    Ok((Cow::Borrowed(&text[1..1 + end]), &text[2 + end..]))
}

// scan_escape reads one escaped character. The usize is the
// whole escape's byte length.
fn scan_escape(text: &str) -> Result<(char, usize), String> {
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'\\' {
        return Err("cannot parse escape".into());
    }
    match bytes[1] {
        b'"' | b'\\' => Ok((bytes[1] as char, 2)),
        b'n' => Ok(('\n', 2)),
        b'r' => Ok(('\r', 2)),
        b't' => Ok(('\t', 2)),
        b'b' => Ok(('\u{8}', 2)),
        b'f' => Ok(('\u{c}', 2)),
        b'u' => scan_unicode(text),
        b'x' => {
            // Two hex digits must follow: four bytes needed.
            if bytes.len() >= 4 {
                if let Ok(v) = parse_hex(&text[2..4]) {
                    if let Some(r) = char::from_u32(v) {
                        return Ok((r, 4));
                    }
                }
            }
            Err(r"cannot parse \x escape".into())
        }
        c => Err(format!(r"cannot parse escape \{}", c as char)),
    }
}

// scan_unicode reads one \u escape, both \u0000 and \u{...}.
fn scan_unicode(text: &str) -> Result<(char, usize), String> {
    let bytes = text.as_bytes();
    let mut hex: Option<&str> = None;
    let mut n = 0;
    if bytes.len() >= 5 && bytes[2] == b'{' {
        // \u{0}
        if let Some(i) = text.find('}') {
            if i > 3 && i < 10 {
                n = i + 1;
                hex = Some(&text[3..i]);
            }
        }
    } else if bytes.len() >= 6 {
        // \u0000
        n = 6;
        hex = Some(&text[2..6]);
    }
    if n > 0 {
        if let Ok(v) = parse_hex(hex.unwrap()) {
            if is_unicode_escape(v) {
                if let Some(r) = char::from_u32(v) {
                    return Ok((r, n));
                }
            }
        }
    }
    Err(r"cannot parse \u escape".into())
}

fn is_unicode_escape(v: u32) -> bool {
    v <= 0x10FFFF && !(0xD800..=0xDFFF).contains(&v)
}

fn parse_hex(s: &str) -> Result<u32, ()> {
    if s.is_empty() {
        return Err(());
    }
    let mut v: u32 = 0;
    for b in s.bytes() {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return Err(()),
        };
        v = v.checked_mul(16).and_then(|v| v.checked_add(d as u32)).ok_or(())?;
    }
    Ok(v)
}

fn scan_word(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    let mut cut = 0;
    while cut < bytes.len() && !b" \t,]}.:".contains(&bytes[cut]) {
        cut += 1;
    }
    (&text[..cut], &text[cut..])
}

fn is_bare_rune(r: char) -> bool {
    r == '-' || r == '_' || r.is_ascii_alphanumeric()
}

fn digit_val(c: u8) -> i32 {
    match c {
        b'0'..=b'9' => (c - b'0') as i32,
        b'a'..=b'f' => (c - b'a') as i32 + 10,
        b'A'..=b'F' => (c - b'A') as i32 + 10,
        _ => -1,
    }
}

// scan_scalar_token reads the maximal token that could spell a
// scalar; the caller tells number, keyword, or bare word.
fn scan_scalar_token(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    let mut cut = 0;
    if cut < bytes.len() && bytes[cut] == b'-' {
        cut += 1;
    }
    while cut < bytes.len() {
        let c = bytes[cut];
        if c.is_ascii_digit() || c.is_ascii_alphanumeric() || c == b'.' {
            cut += 1;
        } else if (c == b'+' || c == b'-') && cut > 0 && (bytes[cut - 1] == b'e' || bytes[cut - 1] == b'E') {
            cut += 1;
        } else {
            return (&text[..cut], &text[cut..]);
        }
    }
    (text, "")
}

// is_decimal_shape matches the grammar's decimal spellings:
// numint [ "." 1*DIGIT ] [ "e" ["+" / "-"] 1*DIGIT ]. The
// redundant leading zero gets its own message.
fn is_decimal_shape(body: &str) -> bool {
    let mut mantissa = body;
    if let Some(i) = body.find('e') {
        mantissa = &body[..i];
        let mut exp = &body[i + 1..];
        if exp.is_empty() {
            return false;
        }
        if exp.starts_with('+') || exp.starts_with('-') {
            exp = &exp[1..];
        }
        if !all_digits(exp) {
            return false;
        }
    }
    let mut int_part = mantissa;
    if let Some(i) = mantissa.find('.') {
        int_part = &mantissa[..i];
        if !all_digits(&mantissa[i + 1..]) {
            return false;
        }
    }
    !int_part.is_empty() && all_digits(int_part)
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

// scan_number validates a number-shaped token and returns its
// pre-encoded json. A bare word is never a value; based forms
// convert to decimal on the spot.
fn scan_number(tok: &str, pt: Pos) -> Result<String, Error> {
    let body = tok.strip_prefix('-').unwrap_or(tok);
    let first = body.as_bytes().first().copied().unwrap_or(0);
    if body.is_empty() || !first.is_ascii_digit() {
        return Err(err_at(pt, format!("cannot parse unquoted value `{}`", escape(tok))));
    }
    let bytes = body.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        for (marker, base) in [(b'x', 16u32), (b'o', 8), (b'b', 2)] {
            if marker != bytes[1] {
                continue;
            }
            let digits = &body[2..];
            for c in digits.bytes() {
                let v = digit_val(c);
                if v < 0 || v as u32 >= base {
                    return Err(err_at(pt, format!("cannot parse number `{}`", escape(tok))));
                }
            }
            let mut json = base_to_decimal(digits, base);
            if body.len() != tok.len() {
                json = format!("-{json}"); // the sign the scanner cut off
            }
            return Ok(json);
        }
    }
    if body.contains('E') {
        return Err(err_at(pt, format!("cannot parse number `{}`: uppercase exponent", escape(tok))));
    }
    if bytes.len() > 1 && bytes[0] == b'0' && bytes[1].is_ascii_digit() {
        return Err(err_at(pt, format!("cannot parse number `{}`: redundant leading zero", escape(tok))));
    }
    if !is_decimal_shape(body) {
        return Err(err_at(pt, format!("cannot parse number `{}`", escape(tok))));
    }
    Ok(tok.to_string())
}

// base_to_decimal converts an already-validated digit string to
// decimal by repeated division by 10: one decimal digit per pass,
// least significant first.
fn base_to_decimal(digits: &str, base: u32) -> String {
    let mut out = String::new();
    let mut digits = digits.to_string();
    while !digits.is_empty() {
        let mut carry = 0u32;
        let mut quot: Vec<u8> = Vec::new();
        for c in digits.bytes() {
            let cur = carry * base + digit_val(c) as u32;
            let q = cur / 10;
            if q < 10 {
                quot.push(b'0' + q as u8);
            } else {
                quot.push(b'a' + (q - 10) as u8);
            }
            carry = cur % 10;
        }
        // Strip the quotient's leading zeros; an empty quotient
        // ends the loop.
        let mut i = 0;
        while i < quot.len() && quot[i] == b'0' {
            i += 1;
        }
        digits = String::from_utf8_lossy(&quot[i..]).into_owned();
        out.push((b'0' + carry as u8) as char);
    }
    out.chars().rev().collect()
}

//
// The json encoding of the leaf values
//

fn write_json_string(b: &mut String, s: &str) {
    b.push('"');
    write_escaped_json(b, s);
    b.push('"');
}

fn write_escaped_json(b: &mut String, s: &str) {
    let bytes = s.as_bytes();
    let mut start = 0;
    for i in 0..bytes.len() {
        let c = bytes[i];
        if c >= 0x20 && c != b'"' && c != b'\\' {
            continue;
        }
        if start < i {
            b.push_str(&s[start..i]);
        }
        match c {
            b'"' => b.push_str("\\\""),
            b'\\' => b.push_str("\\\\"),
            b'\n' => b.push_str("\\n"),
            b'\r' => b.push_str("\\r"),
            b'\t' => b.push_str("\\t"),
            0x08 => b.push_str("\\b"),
            0x0c => b.push_str("\\f"),
            c => {
                let _ = write!(b, "\\u{:04x}", c);
            }
        }
        start = i + 1;
    }
    b.push_str(&s[start..]);
}

// write_reescaped_json unescapes s and reescapes as json in one pass.
fn write_reescaped_json(b: &mut String, s: &str) -> Result<(), ScanError> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\\' {
                i += 1;
            }
            write_escaped_json(b, &s[start..i]);
            continue;
        }
        if i + 1 >= bytes.len() {
            return Err(ScanError::at(i, "cannot end line on an escape"));
        }
        let (r, n) = scan_escape(&s[i..]).map_err(|e| ScanError::at(i, e))?;
        if (r as u32) < 0x80 {
            match r as u8 {
                b'"' => b.push_str("\\\""),
                b'\\' => b.push_str("\\\\"),
                b'\n' => b.push_str("\\n"),
                b'\r' => b.push_str("\\r"),
                b'\t' => b.push_str("\\t"),
                0x08 => b.push_str("\\b"),
                0x0c => b.push_str("\\f"),
                c if c < 0x20 => {
                    let _ = write!(b, "\\u{:04x}", c);
                }
                c => b.push(c as char),
            }
        } else {
            b.push(r);
        }
        i += n;
    }
    Ok(())
}

fn json_string(s: &str) -> String {
    if !needs_escape(s) {
        return format!("\"{s}\"");
    }
    let mut b = String::with_capacity(s.len() + 2);
    write_json_string(&mut b, s);
    b
}

fn needs_escape(s: &str) -> bool {
    s.bytes().any(|b| b < 0x20 || b == b'"' || b == b'\\')
}

//
// The cnf quote spelling, for the error messages that name a key.
//

// quote returns s in the q family or an alternate if that's not
// possible. When force is set, unquoted strings are promoted.
fn quote(s: &str, mut q: QuoteKind, force: bool) -> String {
    match q {
        QuoteKind::Unquoted => {
            if !force && is_bare_word(s) {
                return s.to_string();
            }
            q = QuoteKind::DoubleQuoted;
            if s.contains('"') || s.contains('\\') {
                // The single family spells both raw, the cleaner
                // spelling - when it holds the rest of the content.
                q = QuoteKind::SingleQuoted;
                if s.contains('\'') || s.contains('\n') {
                    q = QuoteKind::DoubleQuoted;
                }
            }
        }
        QuoteKind::SingleQuoted => {
            if s.contains('\'') || s.contains('\n') {
                q = QuoteKind::DoubleQuoted;
            }
        }
        QuoteKind::DoubleQuoted => {}
    }
    match q {
        QuoteKind::DoubleQuoted => {
            let mut b = String::from("\"");
            for r in s.chars() {
                b.push_str(&escape_cnf(r));
            }
            b.push('"');
            b
        }
        QuoteKind::SingleQuoted => format!("'{s}'"),
        QuoteKind::Unquoted => s.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum QuoteKind {
    #[default]
    Unquoted,     // bare word
    DoubleQuoted, // "..."
    SingleQuoted, // '...'
}

// is_bare_word reports whether s can spell as one: non-empty,
// every rune in the bare set.
fn is_bare_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_bare_rune)
}

// quote_key force-quotes one segment for an error message.
fn quote_key(seg: &Seg<'_>) -> String {
    quote(&seg.key, seg.quote, true)
}

// quote_key_path force-quotes a key path for an error message.
fn quote_key_path(path: &[Seg<'_>]) -> String {
    if path.len() == 1 {
        return quote_key(&path[0]);
    }
    let mut b = String::from("`");
    for (i, k) in path.iter().enumerate() {
        if i > 0 {
            b.push('.');
        }
        b.push_str(&quote(&k.key, k.quote, false));
    }
    b.push('`');
    b
}

// redefine_path reports a second definition, anchored at the
// redefining segment and naming that segment alone: `b`, not
// the path that reaches it.
fn redefine_path(seg: &Seg<'_>) -> Error {
    err_at(seg.pos, format!("cannot redefine previous key {}", quote_key(seg)))
}

// expand_path_err reports a map key's second expansion, anchored
// at the re-expanding segment and naming that segment alone.
fn expand_path_err(seg: &Seg<'_>) -> Error {
    err_at(seg.pos, format!("cannot expand map key {} more than once", quote_key(seg)))
}

// escape_cnf spells one rune inside a double-quoted cnf string:
// the closed escape set, \xNN canonical for the C0 controls.
fn escape_cnf(r: char) -> String {
    match r {
        '"' => "\\\"".into(),
        '\\' => "\\\\".into(),
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        '\u{8}' => "\\b".into(),
        '\u{c}' => "\\f".into(),
        r if (r as u32) < 0x20 => format!("\\x{:02x}", r as u32),
        '\u{2028}' | '\u{2029}' => format!("\\u{:04x}", r as u32),
        r => r.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trim_json(text: &str) -> String {
        // The fixture is indented for reading, to_json emits compact
        // json, and a json string holds no newline, so the trim
        // touches layout only.
        let mut out = String::new();
        for line in text.split('\n') {
            let line = line.trim();
            if !line.is_empty() {
                out.push_str(line);
            }
        }
        out
    }

    // The smoke fixture is the spec's own Example, holding most constructs.
    const CNF_STR: &str = r#"
key: "value"
nested.key: "another value"

map:
    inline: {a: 1, b: 2}

    block:
        a: 1
        b: 2

    block.c: 3

list:
    inline: [1, 2, 3]

    block.strings:
        - "item 1"
        - "item 2"

    block.maps:
        - label: "map 1"
        - label: "map 2"

    block.list-of-maps:
        ---
        list1.map1:
            a: 1
            b: 2
        list1.map2:
            c: 3
            d: 4
        ---
        list2.map1:
            a: 1
            b: 2
        ---

    block.list-of-lists:
        ---
        - "list 1 item 1"
        - "list 1 item 2"
        ---
        - "list 2 item 1"
        ---

string:
    double: "line 1\nline 2\nline 3"

    # One line, no escaping, literal slash
    single: 'line 1\nline 1 still'

    # Three lines, with escaping support.
    fence.double.inline: """line 1\nLine 2\nline 3\n"""
    fence.double.block:
        """
        line 1\nline \
        2
        line 3
        """

    # One line, no escaping, literal slash
    fence.single.inline: '''line 1\nline 1 still'''
    fence.single.block:
        '''
        line 1\n
        line 2
        '''

numbers:
    int: 123
    float: 1.23
    hex: 0x123
    octal: 0o777
    binary: 0b011
    exponent: 1e-23
"#;

    const JSON_STR: &str = r#"
{
    "key":"value",
    "nested":{"key":"another value"},
    "map":{
        "inline":{"a":1,"b":2},
        "block":{"a":1,"b":2,"c":3}
    },
    "list":{
        "inline":[1,2,3],
        "block":{
            "strings":["item 1","item 2"],
            "maps":[{"label":"map 1"},{"label":"map 2"}],
            "list-of-maps":[
                {"list1":{"map1":{"a":1,"b":2},"map2":{"c":3,"d":4}}},
                {"list2":{"map1":{"a":1,"b":2}}}
            ],
            "list-of-lists":[
                ["list 1 item 1","list 1 item 2"],
                ["list 2 item 1"]
            ]
        }
    },
    "string":{
        "double":"line 1\nline 2\nline 3",
        "single":"line 1\\nline 1 still",
        "fence":{
            "double":{
                "inline":"line 1\nLine 2\nline 3\n",
                "block":"line 1\nline 2\nline 3\n"
            },
            "single":{
                "inline":"line 1\\nline 1 still",
                "block":"line 1\\n\nline 2\n"
            }
        }
    },
    "numbers":{
        "int":123,
        "float":1.23,
        "hex":291,
        "octal":511,
        "binary":3,
        "exponent":1e-23
    }
}
"#;

    const CNF_ERRORS: &[(&str, &str)] = &[
        (r#"a: "x"#, "1:4: cannot find closing quote"),
        ("a: {b: 1\n", "1:4: cannot find closing `}`"),
        ("a: 1\n\tb: 2\n", "2:2: cannot mix indentation prefixes"),
        ("a:\n b:\n\t\tc: 1\n", "3:3: cannot mix indentation prefixes"),
        ("a:\n    \"\"\"\ncontent\n", "3:1: cannot close fence at shallower prefix"),
        ("1\nbogus\n", "2:1: cannot parse content after top-level value: `bogus`"),
        ("a 1\n", "1:1: cannot parse unquoted value `a`"),
        ("a: bare\n", "1:4: cannot parse unquoted value `bare`"),
        ("a..b: 1\n", "1:3: cannot parse empty key segment"),
        ("a .b: 1\n", "1:2: cannot parse space in path"),
        ("a:\n", "1:1: cannot omit the value for \"a\""),
        ("a:\n    b.c:\n", "2:5: cannot omit the value for `b.c`"),
        ("a: 1\na: 2\n", "2:1: cannot redefine previous key \"a\""),
        ("a: 1\na.b: 2\n", "2:1: cannot redefine previous key \"a\""),
        ("a: {b: 1}\na: {c: 2}\n", "2:1: cannot expand map key \"a\" more than once"),
        ("a: \"x\\dy\"\n", "1:6: cannot parse escape \\d"),
        ("a: 02\n", "1:4: cannot parse number `02`: redundant leading zero"),
        ("a: 1E5\n", "1:4: cannot parse number `1E5`: uppercase exponent"),
    ];

    #[test]
    fn example() {
        let got = to_json(CNF_STR, None).expect("unexpected error");
        assert_eq!(got, trim_json(JSON_STR));
    }

    #[test]
    fn errors() {
        for (cnf, error) in CNF_ERRORS {
            let got = to_json(cnf, None).expect_err(&format!("no error for {cnf:?}"));
            assert_eq!(got.to_string(), *error, "for {cnf:?}");
        }
    }

    #[test]
    fn escape_str() {
        assert_eq!(escape("a\"b\nc"), "a\\\"b\\nc");
    }
}
