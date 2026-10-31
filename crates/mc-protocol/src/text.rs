//! Minimalny komponent tekstu (JSON w login/status, NBT w configuration/play od 1.20.3).

use crate::nbt::{Compound, Tag};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Text {
    pub text: Option<String>,
    pub translate: Option<String>,
    pub with: Vec<Text>,
    pub color: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub extra: Vec<Text>,
    /// Dodatkowe pola stylu (np. clickEvent/hoverEvent/insertion), wyłącznie w kodowaniu NBT.
    pub style_nbt: Vec<(String, Tag)>,
}

impl Text {
    pub fn plain(s: impl Into<String>) -> Self {
        Self { text: Some(s.into()), ..Default::default() }
    }
    pub fn translatable(key: impl Into<String>) -> Self {
        Self { translate: Some(key.into()), ..Default::default() }
    }
    pub fn with_args(mut self, args: Vec<Text>) -> Self {
        self.with = args;
        self
    }
    pub fn color(mut self, c: &str) -> Self {
        self.color = Some(c.to_owned());
        self
    }

    /// Odpowiednik `Component.tryCollapseToString()`: literał bez stylu i dzieci.
    fn collapsed(&self) -> Option<&str> {
        match (&self.text, &self.translate) {
            (Some(t), None)
                if self.with.is_empty()
                    && self.color.is_none()
                    && self.bold.is_none()
                    && self.italic.is_none()
                    && self.extra.is_empty()
                    && self.style_nbt.is_empty() =>
            {
                Some(t)
            }
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map, Value};
        if let Some(s) = self.collapsed() {
            return Value::String(s.to_owned());
        }
        let mut m = Map::new();
        if let Some(t) = &self.text {
            m.insert("text".into(), Value::String(t.clone()));
        }
        if let Some(t) = &self.translate {
            m.insert("translate".into(), Value::String(t.clone()));
        }
        if !self.with.is_empty() {
            m.insert("with".into(), Value::Array(self.with.iter().map(Text::to_json).collect()));
        }
        if let Some(c) = &self.color {
            m.insert("color".into(), Value::String(c.clone()));
        }
        if let Some(b) = self.bold {
            m.insert("bold".into(), Value::Bool(b));
        }
        if let Some(b) = self.italic {
            m.insert("italic".into(), Value::Bool(b));
        }
        if !self.extra.is_empty() {
            m.insert("extra".into(), Value::Array(self.extra.iter().map(Text::to_json).collect()));
        }
        Value::Object(m)
    }

    pub fn to_nbt(&self) -> Tag {
        if let Some(s) = self.collapsed() {
            return Tag::string(s);
        }
        let mut c = Compound::new();
        if let Some(t) = &self.text {
            c.put("text", Tag::string(t.clone()));
        }
        if let Some(t) = &self.translate {
            c.put("translate", Tag::string(t.clone()));
        }
        if !self.with.is_empty() {
            c.put("with", Tag::List(self.with.iter().map(Text::to_nbt).collect()));
        }
        if let Some(col) = &self.color {
            c.put("color", Tag::string(col.clone()));
        }
        if let Some(b) = self.bold {
            c.put("bold", Tag::Byte(b as i8));
        }
        if let Some(b) = self.italic {
            c.put("italic", Tag::Byte(b as i8));
        }
        if !self.extra.is_empty() {
            c.put("extra", Tag::List(self.extra.iter().map(Text::to_nbt).collect()));
        }
        for (k, v) in &self.style_nbt {
            c.put(k.clone(), v.clone());
        }
        Tag::Compound(c)
    }
}
