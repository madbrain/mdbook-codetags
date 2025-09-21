use std::{collections::HashMap, ffi::OsStr, fs::File, io::{BufRead, BufReader, Error}, ops::Not, path::Path};

use mdbook::{preprocess::Preprocessor, BookItem};
use regex::Regex;
use walkdir::WalkDir;
use lazy_static::lazy_static;

use crate::config::Configuration;

struct CodeBook {
    chapters: Vec<Chapter>
}

impl CodeBook {
    fn find_chapter(&self, name: &str) -> Option<&Chapter> {
        return self.chapters.iter().find(|c|c.name == name);
    }

    fn find_code_tag<'a>(&'a self, chapter: &str, name: &str) -> Option<&'a CodeTag> {
        self.find_chapter(chapter).and_then(|chapter|{
            // special case to override omit 
            self.chapters.last().unwrap().find_code_tag(name).or_else(||chapter.find_code_tag(name))
        })
    }
}

struct Chapter {
    name: String,
    code_tags: Vec<CodeTag>
}


impl Chapter {
    fn find_code_tag(&self, name: &str) -> Option<&CodeTag> {
        return self.code_tags.iter().find(|c|c.name == name);
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
struct CodeTag {
    chapter: usize,
    name: String,
    index: u32,
    no_location: bool,
    before_count: u32,
    after_count: u32
}

impl CodeTag {
    fn is_before(&self, other: &CodeTag) -> bool {
        if self.chapter != other.chapter {
            return self.chapter < other.chapter
        }
        return self.index < other.index
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Location {
    parent: Option<Box<Location>>,
    kind: String,
    name: Option<String>,
    is_function_declaration: bool
}

impl Location {
    fn to_html(&self, preceding: Option<&Location>, has_removed: bool) -> Vec<String> {
        let mut result = Vec::new();
        self.recurse(&mut result, preceding, has_removed);
        return result;
    }

    fn recurse(&self, result: &mut Vec<String>, preceding: Option<&Location>, has_removed: bool) {
        if let Some(parent) = &self.parent {
            parent.recurse(result, preceding, has_removed);
        }
        if self.kind == "file" {
            result.push(format!("<em>{}</em>", self.name.as_ref().unwrap().clone()));
        } else if self.kind == "new" {
            result.push(String::from("create new file"));
        } else if self.kind == "top" {
            result.push(String::from("add to top of file"));
        } else if self.kind == "class" { // TODO should more generic to all types
            result.push(String::from(format!("in class <em>{}</em>", self.name.as_ref().unwrap().clone())));
        } else if self.is_function() && preceding.map_or(false, |p| p == self) {
            result.push(format!("in <em>{}</em>()", self.name.as_ref().unwrap()));
        } else if self.is_function() && has_removed {
            result.push(format!("{} <em>{}</em>()", self.kind, self.name.as_ref().unwrap()));
        } else if self.parent.as_ref().map(|p| p.as_ref()) == preceding && !preceding.map_or(false, |p|p.is_file()) {
            result.push(format!("in {} <em>{}</em>", preceding.unwrap().kind, preceding.unwrap().name.as_ref().unwrap()));
        } else if preceding.map_or(false, |p|p == self) && !self.is_file() {
            result.push(format!("in {} <em>$name</em>", self.kind));
        } else if preceding.map_or(false, |p| p.is_function()) {
            result.push(format!("add after <em>{}</em>()", preceding.unwrap().name.as_ref().unwrap()));
        } else if !preceding.map_or(true, |p| p.is_file()) {
            result.push(format!("add after {} <em>{}</em>", preceding.unwrap().kind, preceding.unwrap().name.as_ref().unwrap()));
        }
    }

    fn is_file(&self) -> bool {
        return self.kind == "file";
    }

    fn is_function(&self) -> bool {
        return self.kind == "constructor" || self.kind == "function" || self.kind == "method";
    }

    fn depth(&self) -> usize {
        let mut current= Some(self);
        let mut result = 0;
        while let Some(c) = current  {
            result += 1;
            current = if let Some(x) = &c.parent {
                Some(&*x)
            } else {
                None
            }
        }
        return result;
    }
    
    fn pop_to_depth(&self, depth: usize) -> Location {
        let mut locations: Vec<&Location> = Vec::new();
        let mut current = Some(self);
        while let Some(c) = current {
            locations.push(c);
            current = if let Some(x) = &c.parent {
                Some(&*x)
            } else {
                None
            }
        }

        // If we are already shallower, there is nothing to pop.
        if locations.len() < depth + 1 {
            return self.clone();
        }

        return locations[locations.len() - depth - 1].clone();
    }
}

struct Snippet {
    code_tag: CodeTag,
    location: Option<Location>,
    preceding_location: Option<Location>,
    first_line: usize,
    last_line: usize,
    context_before: Vec<String>,
    context_after: Vec<String>,
    added: Vec<String>,
    removed: Vec<String>
}

impl Snippet {
    fn new<'b>(code_tag: &CodeTag) -> Self {
        return Snippet {
            code_tag: code_tag.clone(),
            location: None,
            preceding_location: None,
            first_line: 0,
            last_line: 0,
            context_before: Vec::new(),
            context_after: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
        }
    }

    fn add_line(&mut self, line_index: usize, line: &SourceLine) {
        if self.added.is_empty() {
            self.location = Some(line.location.clone());
            self.first_line = line_index;
        }
        self.added.push(line.content.clone());
        self.last_line = line_index;
    }

    fn remove_line(&mut self, line_index: usize, line: &SourceLine) {
        self.removed.push(line.content.clone());
        self.last_line = line_index;
    }

    fn compute_context(&mut self, file: &SourceFile) {
        for ii in 0 .. self.first_line {
            let i = self.first_line - 1 - ii;
            if self.context_before.len() >= self.code_tag.before_count as usize {
                break
            }
            let line = &file.lines[i];
            if !line.is_present_at(&self.code_tag) {
                continue
            } 
            self.context_before.insert(0, line.content.clone());
        }

        for i in self.last_line + 1..file.lines.len() {
            if self.context_after.len() >= self.code_tag.after_count as usize {
                break
            }
            let line = &file.lines[i];
            if line.is_present_at(&self.code_tag) {
                self.context_after.push(line.content.clone());
            }
        }

        // println!("COMPUTE CONTEXT {} -> {:?}", self.code_tag.name, self.location);

        let mut checked_lines = 0;
        for ii in 0..self.first_line {
            let i = self.first_line - 1 - ii;
            if checked_lines > 4 { // TODO find a better way to stop iteration
                break
            }
            let line = &file.lines[i];
            if !line.is_present_at(&self.code_tag) {
                continue;
            }
            checked_lines += 1;

            // Store the most precise preceding location we find.
            if self.preceding_location.as_ref().map_or(true,|p| line.location.depth() > p.depth()) {
                self.preceding_location = Some(line.location.clone());
            }
        }
        // println!("PRE {:?}", self.preceding_location);

        let mut has_code_before = self.context_before.is_empty().not();
        let mut has_code_after = self.context_after.is_empty().not();
        for ii in 0 .. self.first_line {
            let i = self.first_line - 1 - ii;
            if has_code_before {
                break
            }
            has_code_before = file.lines[i].is_present_at(&self.code_tag);
        }

        for i in self.last_line + 1..file.lines.len() {
            if has_code_after {
                break
            }
            has_code_after = file.lines[i].is_present_at(&self.code_tag);
        }

        if !has_code_before {
            self.location = Some(Location {
                parent: self.location.as_ref().map(|x| Box::new(x.clone())),
                kind: String::from(if has_code_after { "top" } else { "new" }),
                name: None,
                is_function_declaration: false
            });
        }
    }
}

struct SourceLine<'a> {
    content: String,
    location: Location,
    start: &'a CodeTag,
    end: Option<&'a CodeTag>
}

impl SourceLine<'_> {
    fn is_present_at(&self, tag: &CodeTag) -> bool {
        if tag.is_before(self.start) {
            return false
        }
        if self.end.is_some() && tag.is_before(self.end.unwrap()).not() {
            return false
        } 
        return true
    }
}

struct SourceFile<'a> {
    lines: Vec<SourceLine<'a>>
}

#[derive(Debug)]
struct ParseState<'a> {
    start: &'a CodeTag,
    end: Option<&'a CodeTag>
}

struct SourceFileParser<'a> {
    code_book: &'a CodeBook,
    states: Vec<ParseState<'a>>,
    location: Location
}

lazy_static!{
    pub static ref START_RE: Regex = Regex::new("^//> ([A-Z][A-Za-z\\s]+\\s+)?([-a-z0-9]+)$").unwrap();
    pub static ref END_RE: Regex = Regex::new("^//< ([A-Z][A-Za-z\\s]+\\s+)?([-a-z0-9]+)$").unwrap();
    pub static ref START_BLOCK_RE: Regex = Regex::new("^/\\* ([A-Z][A-Za-z\\s]+) ([-a-z0-9]+) < ([A-Z][A-Za-z\\s]+) ([-a-z0-9]+)$").unwrap();

    pub static ref CONSTRUCTOR_PATTERN: Regex = Regex::new("^  ([A-Z][a-z]\\w+)\\(").unwrap();
    pub static ref FUNCTION_PATTERN: Regex = Regex::new("(\\w+)>*\\*? (\\w+)\\(([^)]*)").unwrap();
    pub static ref VARIABLE_PATTERN: Regex = Regex::new("^\\w+\\*? (\\w+)(;| = )").unwrap();
    pub static ref TYPE_PATTERN: Regex = Regex::new("(public )?(abstract )?(class|enum|interface) ([A-Z]\\w+).*").unwrap();

    pub static ref KEYWORDS: Vec<&'static str> = vec!("new", "return", "throw");
    
    // pub static ref STRUCT_PATTERN: Regex = Regex::new("^struct (\\w+)? \\{$").unwrap();
    // pub static ref NAMED_TYPEDEF_PATTERN: Regex = Regex::new("^typedef (enum|struct|union) (\\w+) \\{$").unwrap();
    // pub static ref UNNAMED_TYPEDEF_PATTERN: Regex = Regex::new("^typedef (enum|struct|union) \\{$").unwrap();
    // pub static ref TYPEDEF_NAME_PATTERN: Regex = Regex::new("^} (\\w+);$").unwrap();
}

impl<'x> SourceFileParser<'x> {

    fn new<'a, 'b>(code_book: &'a CodeBook) -> SourceFileParser<'b> where 'a: 'b {
        return SourceFileParser {
            code_book: code_book,
            states: Vec::new(),
            location: Location {
                parent: None,
                kind: String::new(),
                name: None,
                is_function_declaration: false
            }
        }
    }

    fn parse_source_file<'b>(&mut self, path: &Path, source_dir: &Path) -> Result<SourceFile<'b>, Error> where 'x: 'b {
        let relative_path = path.strip_prefix(source_dir).unwrap();
        // println!("SOURCE {}", relative_path.display());
        self.location = Location {
            parent: None,
            kind: String::from("file"),
            name: Some(String::from(relative_path.to_str().unwrap())),
            is_function_declaration: false            
        };

        let input = File::open(path)?;
        let buffered = BufReader::new(input);
        let mut source_file = SourceFile {
            lines: Vec::new()
        };
        
        self.states.clear();
        let lines: Vec<String> = buffered.lines().map(|l|l.unwrap()).collect(); 
        for (i, line) in lines.iter().enumerate() {
            // println!("LINE '{}'", line);
            self.update_location_before(&line, lines.get(i+1));
            if !self.update_state(line.as_str()) {
                let state = self.states.last().unwrap();
                source_file.lines.push(SourceLine {
                    content: line.clone(),
                    location: self.location.clone(),
                    start: state.start,
                    end: state.end
                });
            }
            self.update_location_after(&line);
        }
        Ok(source_file)
    }

    fn update_location_before(&mut self, line: &String, next_line: Option<&String>) {
        if let Some(c) = FUNCTION_PATTERN.captures(line) {
            if !KEYWORDS.contains(&c.get(1).unwrap().as_str()) {
                // Hack. Don't get caught by comments or string literals.
                if !line.contains("//") && !line.contains('"') {
                    let mut is_function_declaration = line.ends_with(";");

                    // Hack: Handle multi-line declarations.
                    if line.ends_with(",") && next_line.map_or(false, |nl|nl.ends_with(";")) {
                        is_function_declaration = true
                    }

                    self.location = Location {
                        parent: Some(Box::new(self.location.clone())),
                        kind: String::from(if /*file.language == "java"*/ true { "method" } else { "function" }),
                        name: Some(String::from(c.get(2).unwrap().as_str())),
                        //signature = match.groups[3]!!.value,
                        is_function_declaration
                    };
                    return
                }
            }
        }
        
        if let Some(c) = CONSTRUCTOR_PATTERN.captures(line) {
            self.location = Location {
                parent: Some(Box::new(self.location.clone())),
                kind: String::from("constructor"),
                name: Some(String::from(c.get(1).unwrap().as_str())),
                is_function_declaration: false
            };
            return
        }
        if let Some(c) = TYPE_PATTERN.captures(line) {
            // Hack. Don't get caught by comments or string literals.
            if !line.contains("//") && !line.contains('"') {
                self.location = Location {
                    parent: Some(Box::from(self.location.clone())),
                    kind: String::from(c.get(3).unwrap().as_str()),
                    name: Some(String::from(c.get(4).unwrap().as_str())),
                    is_function_declaration: false
                };
            }
            return
        }
        if let Some(c) = VARIABLE_PATTERN.captures(line) {
            self.location = Location{
                parent: Some(Box::from(self.location.clone())),
                kind: String::from("variable"),
                name: Some(String::from(c.get(1).unwrap().as_str())),
                is_function_declaration: false
            };
            return;
        }
    }

    fn update_location_after(&mut self, line: &String) {
        // Use "startsWith" to include lines like "} [aside-marker]".
        if line.starts_with("}") {
            self.location = self.location.pop_to_depth(0);
        } else if line.starts_with("  }") {
            self.location = self.location.pop_to_depth(1)
        } else if line.starts_with("    }") {
            self.location = self.location.pop_to_depth(2)
        }

        // If we reached a function declaration, not a definition, then it's done after one line.
        if self.location.is_function_declaration {
            self.location = *self.location.parent.clone().unwrap();
        }

        // Module variables are only a single line.
        if self.location.kind == "variable" {
            self.location = *self.location.parent.clone().unwrap();
        }

        // Hack. There is a one-line class in Parser.java.
        if line.contains("class ParseError") {
            self.location = *self.location.parent.clone().unwrap();
        }
    }

    fn update_state(&mut self, line: &str) -> bool {
        if let Some(c) = START_RE.captures(line) {
            self.push(c.get(1).map(|x|x.as_str()), c.get(2).unwrap().as_str(), None);
            return true
        }
        if let Some(c) = END_RE.captures(line) {
            // println!("END {}", line);
            let end_name = c.get(2).unwrap().as_str();
            if let Some(chapter_name) = c.get(1).map(|x|x.as_str().trim()) {
                let test_chapter_name = &self.code_book.chapters[self.states.last().unwrap().start.chapter].name;
                if test_chapter_name != "$static$" {
                    assert_eq!(test_chapter_name, chapter_name)
                }
            }
            assert_eq!(self.states.last().unwrap().start.name, end_name);
            assert!(self.states.len() > 0);
            self.pop();
            return true
        }
        if let Some(c) = START_BLOCK_RE.captures(line) {
            let end_chapter = c.get(3).unwrap().as_str();
            let end_name = c.get(4).unwrap().as_str();
            self.push(c.get(1).map(|x|x.as_str()), c.get(2).unwrap().as_str(), 
                Some((end_chapter.trim(), end_name)));
            return true;
        }
        if line.trim() == "*/" {
            self.pop();
            return true
        }
        return false;
    }

    fn pop(&mut self) {
        // let code_tag = self.states.last().unwrap().start;
        // println!("POP BLOCK {}/{}", &self.code_book.chapters[code_tag.chapter].name, code_tag.name);
        self.states.pop();
        // let code_tag = self.states.last().unwrap().start;
        // println!("TOP {}/{}", &self.code_book.chapters[code_tag.chapter].name, code_tag.name);
    }

    fn push(&mut self, start_chapter_name: Option<&str>, start_name: &str, end: Option<(&str, &str)>) {
        let start_chapter_name = start_chapter_name.unwrap_or_else(|| {
            let chapter_name = &self.code_book.chapters[self.states.last().unwrap().start.chapter].name;
            // println!("DEFAULT CHAPTER '{}' / {}", chapter_name, start_name);
            chapter_name
        }).trim();
        let start_code_tag = self.code_book.find_code_tag(start_chapter_name, start_name).unwrap_or_else(|| panic!("unknown start tag: [{:?}] {}/{}", self.location, start_chapter_name, start_name));

        let end_code_tag = end.map(|(end_chapter_name, end_name)| {
            self.code_book.find_code_tag(end_chapter_name, end_name).unwrap()
        });
        self.states.push(ParseState {  start: start_code_tag, end: end_code_tag });
        // print!("PUSH {:?}\n", self.states.last().unwrap());
    }
}

#[derive(Default)]
pub(crate) struct CodeTagsHighlighterPreprocessor;

const CODETAG_RE_STR: &str = r"(?m)^\^code\s+([a-z-]+)\s*(?:\(([^)]*)\))?";
    
impl CodeTagsHighlighterPreprocessor {

    fn collect_code_tags(&self, book: &mdbook::book::Book) -> CodeBook {
        let codetag_re = Regex::new(CODETAG_RE_STR).unwrap();

        let mut chapters: Vec<Chapter> = Vec::new();
    
        for item in book.iter() { 
            if let BookItem::Chapter(chapter) = item {
                let mut index = 0;
                
                for c in codetag_re.captures_iter(&chapter.content) {
                    let id = c.get(1).unwrap().as_str();
                    let mut no_location = false;
                    let mut before_count = 0;
                    let mut after_count = 0;
                    c.get(2)
                        .map(|x|x.as_str()).unwrap_or("")
                        .split(",")
                        .map(|x|x.trim())
                        .for_each(|opt|{
                        if opt == "no location" {
                            no_location = true
                        } else if opt.ends_with(" before") {
                            before_count = opt[..opt.len()-6].trim().parse().unwrap();
                        } else if opt.ends_with(" after") {
                            after_count = opt[..opt.len()-5].trim().parse().unwrap();
                        }
                    });

                    let chapter_index = if let Some(i) = chapters.iter().position(|c|c.name == chapter.name) {
                        i
                    } else {
                        chapters.push(Chapter {
                            name: String::from(chapter.name.clone()),
                            code_tags: Vec::new()
                        });
                        chapters.len() - 1
                    };

                    chapters[chapter_index].code_tags.push(CodeTag {
                        chapter: chapter_index,
                        name: String::from(id),
                        index: index,
                        no_location: no_location,
                        before_count: before_count,
                        after_count: after_count
                    });
                    index += 1;
                }
            }
        }
        chapters.push(Chapter { name: String::from("$static$"), code_tags: vec![
            CodeTag { chapter: chapters.len(), name: String::from("omit"), index: 9998, before_count: 0, after_count: 0, no_location: false },
            CodeTag { chapter: chapters.len(), name: String::from("not-yet"), index: 9999, before_count: 0, after_count: 0, no_location: false }
        ] });
        return CodeBook { chapters: chapters };
    }

    
}

impl Preprocessor for CodeTagsHighlighterPreprocessor {
    fn name(&self) -> &str {
        "codetags"
    }

    fn supports_renderer(&self, renderer: &str) -> bool {
        renderer == "html"
    }

    fn run(&self, ctx: &mdbook::preprocess::PreprocessorContext, mut book: mdbook::book::Book) -> mdbook::errors::Result<mdbook::book::Book> {
        
        let config: Configuration = match ctx.config.get_preprocessor(self.name()) {
            Some(c) => c.try_into().unwrap(),
            None => Configuration::default(),
        };

        let code_book = self.collect_code_tags(&book);
        
        // // <debug>
        // let mut file = std::fs::File::create("dump.txt").unwrap();
        // for chapter in &code_book.chapters {
        //     for code_tag in &chapter.code_tags {
        //         let xx = format!("CODETAG '{}' / {} / {} / {} / {}\n", code_book.chapters[code_tag.chapter].name, code_tag.name, code_tag.no_location, code_tag.before_count, code_tag.after_count);
        //         file.write_all(xx.as_bytes()).unwrap();
        //     }
        // }
        // file.flush().unwrap();
        // // </debug>

        let source_dir = &if config.src_root.is_relative() {
            ctx.root.join(config.src_root)
        } else {
            config.src_root
        };

        let mut snippets: HashMap<&str, Snippet> = HashMap::new();

        for entry in WalkDir::new(source_dir)
                .into_iter()
                .filter_map(|e| e.ok())
                // .filter(|e| e.path().file_name().unwrap() == "Lox.java")
                .filter(|e| e.metadata().unwrap().is_file() && e.path().extension().and_then(OsStr::to_str).unwrap() == "java") {
            let path = entry.path();
            // let metadata = entry.metadata()?;
            // let modified = metadata.modified()?.elapsed()?.as_secs();
            // file.write_all(format!("SOURCE {}\n", path.display()).as_bytes())?;
            
            let mut parser = SourceFileParser::new(&code_book);
            let source_file = parser.parse_source_file(path, source_dir).unwrap();
            let mut local_snippets: HashMap<&str, Snippet> = HashMap::new();
            for (line_index, line) in source_file.lines.iter().enumerate() {
                let start_name = line.start.name.as_str();
                if !local_snippets.contains_key(start_name) {
                    local_snippets.insert(start_name, Snippet::new(line.start));
                }
                let snippet = local_snippets.get_mut(start_name).unwrap();
                snippet.add_line(line_index, line);

                if let Some(end) = line.end {
                    let end_name = end.name.as_str();
                    if !local_snippets.contains_key(end_name) {
                        local_snippets.insert(end_name, Snippet::new(end));
                    }
                    let snippet = local_snippets.get_mut(end_name).unwrap();
                    snippet.remove_line(line_index, line);
                }
            }
            for (_, snippet) in &mut local_snippets {
                snippet.compute_context(&source_file);
            }
            snippets.extend(local_snippets);
        }

        // // <debug>
        // file.flush()?;
        // // </debug>

        let codetag_re = Regex::new(CODETAG_RE_STR).unwrap();
        book.for_each_mut(|item| {
            if let BookItem::Chapter(chapter) = item {
                let mut updated_content = String::with_capacity(chapter.content.len());
                for line in chapter.content.lines() {
                    if let Some(m) = codetag_re.captures(&line) {
                        let id = m.get(1).unwrap().as_str();
                        if let Some(snippet) = snippets.get(id) {
                            updated_content.push_str("<pre>");
                            updated_content.push_str("<code class=\"language-java\">");
                            for line in &snippet.context_before {
                                updated_content.push_str("  ");
                                updated_content.push_str(line);
                                updated_content.push('\n');
                            }
                            for line in &snippet.removed {
                                updated_content.push_str("- ");
                                updated_content.push_str(line);
                                updated_content.push('\n');
                            }
                            for line in &snippet.added {
                                updated_content.push_str("+ ");
                                updated_content.push_str(line);
                                updated_content.push('\n');
                            }
                            for line in &snippet.context_after {
                                updated_content.push_str("  ");
                                updated_content.push_str(line);
                                updated_content.push('\n');
                            }
                            updated_content.push_str("</code>\n");
                            if let Some(location) = &snippet.location {
                                updated_content.push_str("<div class=\"location\">");
                                // updated_content.push_str(format!("<div>{:?}</div> <div>{:?}</div><br>", snippet.preceding_location, snippet.location).as_str());
                                for (index, line) in location.to_html(
                                    snippet.preceding_location.as_ref(),
                                    !snippet.removed.is_empty()
                                ).iter().enumerate() {
                                    if index > 0 {
                                        updated_content.push_str(", ");
                                    }
                                    updated_content.push_str(line);
                                }
                                updated_content.push_str("</div>\n");
                            }
                            updated_content.push_str("</pre>\n");
                        } else {
                            updated_content.push_str(format!("<p>Code tag {} not found</p>\n", id).as_str());
                        }
                    } else {
                        updated_content.push_str(line);
                        updated_content.push('\n');
                    }
                }
                chapter.content = updated_content;
            }
        });

        Ok(book)
    }
}