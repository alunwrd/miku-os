// XML tree for SVG files
//
// Elements, attributes, and text. Namespace prefixes are kept as written
// (`xlink:href`), and the default namespace is ignored. Comments, processing
// instructions, and DOCTYPE are skipped; CDATA is text; predefined entities and
// character references resolve. Malformed input ends the tree where it stops
// parsing.

#[derive(Debug, Default, Clone)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// All the text directly inside, CDATA included
    pub fn text(&self) -> String {
        self.children.iter().filter_map(|n| match n {
            Node::Text(t) => Some(t.as_str()),
            Node::Element(_) => None,
        }).collect()
    }
}

/// The document's root element
pub fn parse(src: &str) -> Option<Element> {
    let mut p = Parser { s: src.as_bytes(), at: 0 };
    let mut stack: Vec<Element> = vec![Element::default()];
    while p.at < p.s.len() {
        if p.rest().starts_with(b"<!--") {
            p.skip_past(b"-->");
        } else if p.rest().starts_with(b"<![CDATA[") {
            p.at += 9;
            let start = p.at;
            p.skip_past(b"]]>");
            let end = p.at.saturating_sub(3).max(start);
            let t = String::from_utf8_lossy(&p.s[start..end]).into_owned();
            stack.last_mut()?.children.push(Node::Text(t));
        } else if p.rest().starts_with(b"<?") {
            p.skip_past(b"?>");
        } else if p.rest().starts_with(b"<!") {
            p.skip_doctype();
        } else if p.rest().starts_with(b"</") {
            p.skip_past(b">");
            if stack.len() > 1 {
                let done = stack.pop()?;
                stack.last_mut()?.children.push(Node::Element(done));
            }
        } else if p.peek() == Some(b'<') {
            p.at += 1;
            let name = p.name();
            let mut el = Element { name, ..Element::default() };
            loop {
                p.space();
                match p.peek() {
                    None => break,
                    Some(b'/') => {
                        p.skip_past(b">");
                        stack.last_mut()?.children.push(Node::Element(el));
                        break;
                    }
                    Some(b'>') => {
                        p.at += 1;
                        stack.push(el);
                        break;
                    }
                    Some(_) => {
                        let k = p.name();
                        if k.is_empty() {
                            p.at += 1;
                            continue;
                        }
                        p.space();
                        let v = if p.peek() == Some(b'=') {
                            p.at += 1;
                            p.space();
                            p.quoted()
                        } else {
                            String::new()
                        };
                        el.attrs.push((k, v));
                    }
                }
                if stack.is_empty() {
                    return None;
                }
            }
        } else {
            let start = p.at;
            while p.peek().is_some_and(|c| c != b'<') {
                p.at += 1;
            }
            let t = unescape(&String::from_utf8_lossy(&p.s[start..p.at]));
            if !t.trim().is_empty() {
                stack.last_mut()?.children.push(Node::Text(t));
            }
        }
    }
    // Closes what is still open at the end of the file
    while stack.len() > 1 {
        let done = stack.pop()?;
        stack.last_mut()?.children.push(Node::Element(done));
    }
    let doc = stack.pop()?;
    doc.children.into_iter().find_map(|n| match n {
        Node::Element(e) => Some(e),
        Node::Text(_) => None,
    })
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &[u8] {
        &self.s[self.at..]
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    fn space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn skip_past(&mut self, end: &[u8]) {
        match self.rest().windows(end.len()).position(|w| w == end) {
            Some(i) => self.at += i + end.len(),
            None => self.at = self.s.len(),
        }
    }

    /// `<!DOCTYPE ...>`, with an internal subset in brackets if it has one
    fn skip_doctype(&mut self) {
        let mut depth = 0;
        while let Some(c) = self.peek() {
            self.at += 1;
            match c {
                b'[' => depth += 1,
                b']' => depth -= 1,
                b'>' if depth <= 0 => return,
                _ => {}
            }
        }
    }

    fn name(&mut self) -> String {
        let start = self.at;
        while self.peek().is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b'=' | b'>' | b'/' | b'<' | b'"' | b'\'')) {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.at]).into_owned()
    }

    fn quoted(&mut self) -> String {
        let Some(q) = self.peek().filter(|c| matches!(c, b'"' | b'\'')) else {
            return self.name();
        };
        self.at += 1;
        let start = self.at;
        while self.peek().is_some_and(|c| c != q) {
            self.at += 1;
        }
        let v = unescape(&String::from_utf8_lossy(&self.s[start..self.at]));
        self.at = (self.at + 1).min(self.s.len());
        v
    }
}

/// The predefined entities and character references, resolved
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name.strip_prefix("#x").or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_svg_reads_as_a_tree() {
        let doc = parse(r#"<?xml version="1.0"?>
            <!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "x" [ <!ENTITY a "b"> ]>
            <!-- made by hand -->
            <svg xmlns="http://www.w3.org/2000/svg" width='22'>
              <style><![CDATA[ .a { color: red } ]]></style>
              <path d="M0 0 &amp; &#65;&#x42;" class="a"/>
              <g><rect/></g>
            </svg>"#).unwrap();
        assert_eq!(doc.name, "svg");
        assert_eq!(doc.attr("width"), Some("22"));
        let kids: Vec<&Element> = doc.elements().collect();
        assert_eq!(kids.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["style", "path", "g"]);
        assert_eq!(kids[0].text().trim(), ".a { color: red }");
        assert_eq!(kids[1].attr("d"), Some("M0 0 & AB"));
        assert_eq!(kids[2].elements().next().unwrap().name, "rect");
    }
}
