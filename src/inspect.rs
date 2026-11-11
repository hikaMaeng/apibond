use crate::config::{Result, valid_id};
use bytes::Bytes;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{collections::HashSet, fmt, ops::Range};

pub struct Fields<'a>(pub Vec<(String, &'a RawValue)>);
impl<'de> Deserialize<'de> for Fields<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Fields<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut fields = Vec::new();
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if key.len() > 512 || fields.len() >= 4096 {
                        return Err(de::Error::custom("too many or oversized JSON keys"));
                    }
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom("duplicate decoded JSON key"));
                    }
                    fields.push((key, map.next_value::<&'de RawValue>()?));
                }
                Ok(Fields(fields))
            }
        }
        d.deserialize_map(Object)
    }
}
impl<'a> Fields<'a> {
    pub fn get(&self, key: &str) -> Option<&'a RawValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
    pub fn string(&self, key: &str) -> Result<Option<String>> {
        match self.get(key) {
            None => Ok(None),
            Some(v) if v.get() == "null" => Ok(None),
            Some(v) => serde_json::from_str::<String>(v.get())
                .map(Some)
                .map_err(|_| format!("{key} must be a string")),
        }
    }
}

#[derive(Default)]
pub struct Metadata {
    pub model: Option<String>,
    pub model_span: Option<Range<usize>>,
    pub session: Option<String>,
    pub previous: Option<String>,
    pub files: Vec<String>,
    pub untracked_state: bool,
}

pub fn inspect(body: &Bytes, user_fallback: bool) -> Result<Metadata> {
    let root: Fields<'_> =
        serde_json::from_slice(body).map_err(|e| format!("invalid JSON: {e}"))?;
    let mut meta = Metadata {
        model: root.string("model")?,
        previous: root.string("previous_response_id")?,
        ..Default::default()
    };
    if let Some(raw) = root.get("model") {
        let start = raw.get().as_ptr() as usize - body.as_ptr() as usize;
        meta.model_span = Some(start..start + raw.get().len());
    }
    if let Some(raw) = root.get("metadata").filter(|v| v.get() != "null") {
        let fields: Fields<'_> = serde_json::from_str(raw.get())
            .map_err(|_| "metadata must be an object with unique keys")?;
        meta.session = fields.string("session_id")?;
        if meta.session.is_none() && user_fallback {
            meta.session = fields.string("user_id")?;
        }
    }
    if meta.session.is_none() && user_fallback {
        meta.session = root.string("user")?;
    }
    for key in ["conversation", "container", "skills"] {
        if root.get(key).is_some_and(|v| v.get() != "null") {
            meta.untracked_state = true;
        }
    }
    if let Some(v) = root.get("prompt").filter(|v| v.get().starts_with('{')) {
        let f: Fields<'_> = serde_json::from_str(v.get()).map_err(|e| e.to_string())?;
        if f.get("id").is_some() {
            meta.untracked_state = true;
        }
    }
    for key in ["input", "messages"] {
        if let Some(v) = root.get(key) {
            walk_content(v, 0, &mut meta)?;
        }
    }
    if let Some(v) = root.get("tools").filter(|v| v.get() != "null") {
        walk_tools(v, &mut meta)?;
    }
    for id in meta
        .model
        .iter()
        .chain(meta.session.iter())
        .chain(meta.previous.iter())
        .chain(meta.files.iter())
    {
        valid_id(id)?;
    }
    meta.files.sort_unstable();
    meta.files.dedup();
    Ok(meta)
}

fn fields(raw: &RawValue) -> Result<Fields<'_>> {
    serde_json::from_str(raw.get()).map_err(|e| e.to_string())
}
fn array(raw: &RawValue) -> Result<Vec<&RawValue>> {
    struct Bounded;
    impl<'de> Visitor<'de> for Bounded {
        type Value = Vec<&'de RawValue>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("array with at most 65536 items")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut input: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut result = Vec::new();
            while let Some(v) = input.next_element::<&'de RawValue>()? {
                if result.len() >= 65536 {
                    return Err(de::Error::custom("too many native array entries"));
                }
                result.push(v);
            }
            Ok(result)
        }
    }
    let mut d = serde_json::Deserializer::from_str(raw.get());
    d.deserialize_seq(Bounded).map_err(|e| e.to_string())
}
fn walk_content(raw: &RawValue, depth: usize, m: &mut Metadata) -> Result<()> {
    if depth > 128 {
        return Err("native content is too deeply nested".into());
    }
    match raw.get().as_bytes().first() {
        Some(b'[') => {
            for item in array(raw)? {
                walk_content(item, depth + 1, m)?;
            }
        }
        Some(b'{') => {
            let f = fields(raw)?;
            let kind = f
                .get("type")
                .and_then(|v| serde_json::from_str::<String>(v.get()).ok());
            if matches!(
                kind.as_deref(),
                Some("input_file" | "input_image" | "container_upload")
            ) && let Some(id) = f.string("file_id")?
            {
                m.files.push(id);
            }
            if kind.as_deref() == Some("item_reference") {
                m.untracked_state = true;
            }
            if let Some(source) = f.get("source").filter(|v| v.get().starts_with('{')) {
                let source = fields(source)?;
                if source.string("type")?.as_deref() == Some("file")
                    && let Some(id) = source.string("file_id")?
                {
                    m.files.push(id);
                }
            }
            if let Some(file) = f.get("file").filter(|v| v.get().starts_with('{'))
                && let Some(id) = fields(file)?.string("file_id")?
            {
                m.files.push(id);
            }
            for key in ["content", "output"] {
                if let Some(child) = f.get(key) {
                    walk_content(child, depth + 1, m)?;
                }
            }
        }
        _ => (),
    }
    if m.files.len() > 4096 {
        return Err("too many file references".into());
    }
    Ok(())
}
fn walk_tools(raw: &RawValue, m: &mut Metadata) -> Result<()> {
    let tools = array(raw)?;
    for tool in tools {
        let f = fields(tool)?;
        match f.string("type")?.as_deref() {
            Some("file_search") => {
                if f.get("vector_store_ids").is_some() {
                    m.untracked_state = true;
                }
            }
            Some("code_interpreter") => {
                if let Some(c) = f.get("container").filter(|v| v.get() != "null") {
                    if c.get().starts_with('"') {
                        m.untracked_state = true;
                    } else if c.get().starts_with('{') {
                        let c = fields(c)?;
                        if let Some(ids) = c.get("file_ids").filter(|v| v.get() != "null") {
                            for id in array(ids)? {
                                if m.files.len() >= 4096 {
                                    return Err("too many file references".into());
                                }
                                let id: String = serde_json::from_str(id.get())
                                    .map_err(|_| "file_ids must be strings")?;
                                valid_id(&id)?;
                                m.files.push(id);
                            }
                        }
                    }
                }
            }
            _ => (),
        }
    }
    Ok(())
}

pub fn body_segments(body: Bytes, span: Option<Range<usize>>, model: &str) -> Vec<Bytes> {
    if let Some(span) = span {
        let replacement = serde_json::to_vec(model).expect("string JSON is infallible");
        if body[span.clone()] == replacement {
            return vec![body];
        }
        vec![
            body.slice(..span.start),
            Bytes::from(replacement),
            body.slice(span.end..),
        ]
    } else {
        vec![body]
    }
}

// Incremental lexical observer: memory depends on nesting/key length, never response length.
#[derive(Default)]
struct Frame {
    object: bool,
    key: String,
    expect_key: bool,
    under_response: bool,
}
#[derive(Default)]
pub struct IdProbe {
    sse: bool,
    line_prefix: Vec<u8>,
    line_data: bool,
    line_skip: bool,
    line_seen: bool,
    frames: Vec<Frame>,
    in_string: bool,
    escape: bool,
    string_buf: Vec<u8>,
    keep_string: bool,
    key_string: bool,
    wanted_string: bool,
    overflow: bool,
    disabled: bool,
}
impl IdProbe {
    pub fn new(sse: bool) -> Self {
        Self {
            sse,
            ..Default::default()
        }
    }
    pub fn feed(&mut self, chunk: &[u8]) -> Option<String> {
        if self.disabled {
            return None;
        }
        for &b in chunk {
            let found = if self.sse {
                self.sse_byte(b)
            } else {
                self.json_byte(b)
            };
            if let Some(id) = found {
                self.disabled = true;
                return Some(id);
            }
        }
        None
    }
    pub fn retained_bytes(&self) -> usize {
        self.line_prefix.capacity()
            + self.string_buf.capacity()
            + self.frames.capacity() * std::mem::size_of::<Frame>()
            + self.frames.iter().map(|f| f.key.capacity()).sum::<usize>()
    }
    fn reset_json(&mut self) {
        self.frames.clear();
        self.in_string = false;
        self.escape = false;
        self.string_buf.clear();
        self.disabled = false;
    }
    fn sse_byte(&mut self, b: u8) -> Option<String> {
        if b == b'\r' {
            return None;
        }
        if b == b'\n' {
            if !self.line_seen {
                self.reset_json();
            }
            self.line_seen = false;
            self.line_data = false;
            self.line_skip = false;
            self.line_prefix.clear();
            return self.json_byte(b'\n');
        }
        self.line_seen = true;
        if self.line_data {
            return self.json_byte(b);
        }
        if self.line_skip {
            return None;
        }
        self.line_prefix.push(b);
        if self.line_prefix == b"data:" {
            self.line_data = true;
            self.line_prefix.clear();
        } else if !b"data:".starts_with(&self.line_prefix) {
            self.line_skip = true;
            self.line_prefix.clear();
        }
        None
    }
    fn json_byte(&mut self, b: u8) -> Option<String> {
        if self.disabled {
            return None;
        }
        if self.in_string {
            if self.keep_string && !self.overflow {
                if self.string_buf.len() < 4096 {
                    self.string_buf.push(b);
                } else {
                    self.overflow = true;
                }
            }
            if self.escape {
                self.escape = false;
                return None;
            }
            if b == b'\\' {
                self.escape = true;
                return None;
            }
            if b != b'"' {
                return None;
            }
            self.in_string = false;
            if self.keep_string
                && !self.overflow
                && let Ok(s) = serde_json::from_slice::<String>(&self.string_buf)
            {
                if self.key_string {
                    if s.len() <= 256
                        && let Some(f) = self.frames.last_mut()
                    {
                        f.key = s;
                    }
                } else if self.wanted_string && valid_id(&s).is_ok() {
                    return Some(s);
                }
            }
            return None;
        }
        match b {
            b'"' => {
                self.in_string = true;
                self.escape = false;
                self.string_buf.clear();
                self.overflow = false;
                self.key_string = self.frames.last().is_some_and(|f| f.object && f.expect_key);
                self.wanted_string = self.frames.last().is_some_and(|f| {
                    f.object
                        && !f.expect_key
                        && f.key == "id"
                        && if self.sse {
                            f.under_response && self.frames.len() == 2
                        } else {
                            self.frames.len() == 1
                        }
                });
                self.keep_string = self.key_string || self.wanted_string;
                if self.keep_string {
                    self.string_buf.push(b'"');
                }
            }
            b'{' | b'[' => {
                if self.frames.len() >= 128 {
                    self.disabled = true;
                    return None;
                }
                let under_response = self.frames.len() == 1
                    && self.frames.last().is_some_and(|f| f.key == "response");
                self.frames.push(Frame {
                    object: b == b'{',
                    expect_key: b == b'{',
                    under_response,
                    ..Default::default()
                });
            }
            b'}' | b']' => {
                self.frames.pop();
            }
            b':' => {
                if let Some(f) = self.frames.last_mut() {
                    f.expect_key = false;
                }
            }
            b',' => {
                if let Some(f) = self.frames.last_mut()
                    && f.object
                {
                    f.expect_key = true;
                    f.key.clear();
                }
            }
            _ => (),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_duplicate_rejected() {
        assert!(
            inspect(
                &Bytes::from_static(br#"{"model":"a","mod\u0065l":"b"}"#),
                false
            )
            .is_err()
        );
        assert_eq!(
            inspect(&Bytes::from_static(br#"{"model":"a\u002db"}"#), false)
                .unwrap()
                .model
                .as_deref(),
            Some("a-b")
        );
    }
    #[test]
    fn preserves_payload_and_ignores_metadata() {
        let b=Bytes::from_static(br#"{ "model" : "alias", "metadata":{"file_id":"fake"}, "messages":[{"content":[{"type":"file","file":{"file_id":"real"}}]}], "unknown":1.2300 }"#);
        let m = inspect(&b, false).unwrap();
        assert_eq!(m.files, vec!["real"]);
        let result = body_segments(b.clone(), m.model_span, "actual").concat();
        assert_eq!(
            result,
            String::from_utf8(b.to_vec())
                .unwrap()
                .replace("\"alias\"", "\"actual\"")
                .as_bytes()
        );
    }
    #[test]
    fn probe_split_and_late_id() {
        let json = format!(
            "{{\"output\":\"{}\",\"id\":\"resp_late\"}}",
            "x".repeat(8 * 1024 * 1024)
        );
        let mut p = IdProbe::new(false);
        let mut found = None;
        for chunk in json.as_bytes().chunks(137) {
            found = found.or_else(|| p.feed(chunk));
        }
        assert_eq!(found.as_deref(), Some("resp_late"));
        assert!(p.retained_bytes() < 32768);
        let sse=b": hi\r\nevent: response.created\r\ndata: {\"type\":\"response.created\",\r\ndata: \"response\":{\"id\":\"resp_abc\"}}\r\n\r\n";
        for size in 1..sse.len() {
            let mut p = IdProbe::new(true);
            let found = sse.chunks(size).find_map(|c| p.feed(c));
            assert_eq!(found.as_deref(), Some("resp_abc"));
        }
    }
}
