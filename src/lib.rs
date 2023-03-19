use std::collections::HashMap;

// TODO: Add the ability to hack into the code or add queuing

use regex::Regex;
use serde_json::Value;
use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use reqwest::header::{HeaderMap, HeaderValue};

lazy_static! {
    static ref ISO_8601_DATE : Regex = Regex::new(r"^\d{4}-\d{2}-\d{2}$")
        .expect("ISO 8601 date regex to be parseable");
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    Deserialization(serde_json::Error, Option<Value>),
    Header(reqwest::header::InvalidHeaderValue),
    ChronoParse(chrono::ParseError),
    NoSuchProperty(String),
    Unknown
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Http(error)
    }
}

impl From<reqwest::header::InvalidHeaderValue> for Error {
    fn from(error: reqwest::header::InvalidHeaderValue) -> Self {
        Error::Header(error)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Error::Deserialization(error, None)
    }
}

impl From<chrono::ParseError> for Error {
    fn from(error: chrono::ParseError) -> Self {
        Error::ChronoParse(error)
    }
}

// TODO: Convert to macro?
// TODO: Investigate if I need to add a case for Some(Value::Null) instead of None
fn parse<T: for<'de> Deserialize<'de>>(key: &str, data: &Value) -> Result<T> {
    Ok(
        serde_json::from_value::<T>(
            data.get(key).ok_or_else(|| Error::NoSuchProperty(key.to_string()))?.clone()
        )?
    )
}

const NOTION_VERSION: &str = "2022-06-28";

fn get_http_client(notion_api_key: &str) -> reqwest::Client {
    let mut headers = HeaderMap::new();
    headers.insert("Authorization", HeaderValue::from_str(&format!("Bearer {notion_api_key}")).expect("bearer token to be parsed into a header"));
    headers.insert("Notion-Version", HeaderValue::from_str(NOTION_VERSION).expect("notion version to be parsed into a header"));
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));

    reqwest::ClientBuilder::new()
        .default_headers(headers)
        .build()
        .expect("to build a valid client out of notion_api_key")
}

#[allow(unused)]
pub struct Client {
    http_client: Rc<reqwest::Client>,
    pub pages: Pages,
    pub blocks: Blocks
}

use std::rc::Rc;

impl<'a> Client {
    pub fn new(notion_api_key: &'a str) -> Self {
        let http_client = Rc::from(get_http_client(notion_api_key));
        
        Client {
            http_client: http_client.clone(),
            pages: Pages {
                http_client: http_client.clone()
            },
            blocks: Blocks {
                http_client: http_client.clone()
            }
        }
    }
}

pub struct PageOptions<'a> {
    pub page_id: &'a str 
}

pub struct Pages {
    http_client: Rc<reqwest::Client>
}

impl Pages {
    pub async fn retrieve<'a>(self, options: PageOptions<'a>) -> Result<Page> {
        let url = format!("https://api.notion.com/v1/pages/{page_id}", page_id = options.page_id);

        let request = self.http_client
            .get(url)
            .send()
            .await?;

        match request.error_for_status_ref() {
            Ok(_) => Ok(request.json().await?),
            Err(error) => {
                println!("Error: {error:#?}");
                println!("Body: {:#?}", request.json::<Value>().await?);
                Err(Error::Http(error))
            }
        }
    }
}

pub struct Blocks {
    http_client: Rc<reqwest::Client>,
}

impl Blocks {
    pub fn children(&self) -> BlockChildren {
        BlockChildren {
            http_client: self.http_client.clone()
        }
    }
}

pub struct BlockChildren {
    http_client: Rc<reqwest::Client>,
}

pub struct BlockChildrenListOptions<'a> {
    pub block_id: &'a str
}

impl BlockChildren {
    pub async fn list<'a>(self, options: BlockChildrenListOptions<'a>) -> Result<QueryResponse<Block>> {
        let url = format!("https://api.notion.com/v1/blocks/{block_id}/children", block_id = options.block_id);

        let request = self.http_client
            .get(&url)
            .send()
            .await?;

        match request.error_for_status_ref() {
            Ok(_) => {
                Ok(request.json().await?)
            },
            Err(error) => {
                println!("Error: {error:#?}");
                println!("Body: {:#?}", request.json::<Value>().await?);
                Err(Error::Http(error))
            }
        }
    }
}

pub struct Databases {
    http_client: Rc<reqwest::Client>,
    // token: String
}

impl Databases {
    pub async fn query(self, options: DatabaseQueryOptions) -> Result<QueryResponse<Page>> {
        let url = format!("https://api.notion.com/v1/databases/{database_id}/query", database_id = options.database_id);

        let request = self.http_client
            .post(url)
            .send()
            .await?;

        match request.error_for_status_ref() {
            Ok(_) => Ok(request.json().await?),
            Err(error) => {
                println!("Error: {error:#?}");
                println!("Body: {:#?}", request.json::<Value>().await?);
                Err(Error::Http(error))
            }
        }
    }
}

pub struct DatabaseQueryOptions {
    pub database_id: String,
    pub filter: Option<Value> // TODO: Implement spec for filter
}


#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub struct Block {
    pub id: String,
    pub parent: Parent,
    pub created_time: DateValue,
    pub last_edited_time: DateValue,
    pub created_by: PartialUser,
    pub last_edited_by: PartialUser,
    pub has_children: bool,
    pub archived: bool,
    pub value: BlockType
}

impl TryFrom<Value> for Block {
    type Error = Error;

    fn try_from(data: Value) -> Result<Block> {
        Ok(
            Block {
                id: parse("id", &data)?,
                parent: parse("parent", &data)?,
                created_time: parse("created_time", &data)?,
                last_edited_time: parse("last_edited_time", &data)?,
                created_by: parse("created_by", &data)?,
                last_edited_by: parse("last_edited_by", &data)?,
                has_children: parse("has_children", &data)?,
                archived: parse("archived", &data)?,
                value: match parse::<String>("type", &data)?.as_str() {
                    "heading_1" => BlockType::Heading1(parse("heading_1", &data)?),
                    "heading_2" => BlockType::Heading2(parse("heading_2", &data)?),
                    "heading_3" => BlockType::Heading3(parse("heading_3", &data)?),
                    "paragraph" => BlockType::Paragraph(parse("paragraph", &data)?),
                    "child_database" => BlockType::ChildDatabase(parse("child_database", &data)?),
                    "child_page" => BlockType::ChildPage(parse("child_page", &data)?),
                    "code" => BlockType::Code(parse("code", &data)?),
                    "bulleted_list_item" => BlockType::BulletedListItem(parse("bulleted_list_item", &data)?),
                    "numbered_list_item" => BlockType::NumberedListItem(parse("numbered_list_item", &data)?),
                    "quote" => BlockType::Quote(parse("quote", &data)?),
                    "callout" => BlockType::Callout(parse("callout", &data)?),
                    "to_do" => BlockType::ToDo(parse("to_do", &data)?),
                    "image" => BlockType::Image(serde_json::from_value(data)?),
                    "column_list" => BlockType::ColumnList(parse("column_list", &data)?),
                    "column" => BlockType::Column(parse("column", &data)?),

                    string => BlockType::Unsupported(string.to_string())
                }
            }
        )
    }
}


#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
pub enum BlockType {
    Paragraph(Paragraph),
    BulletedListItem(ListItem), 
    NumberedListItem(ListItem),
    ToDo(ToDoItem),
    Quote(Quote),
    Callout(Callout),
    ChildPage(ChildPage),
    ChildDatabase(ChildDatabase),
    Heading1(Heading),
    Heading2(Heading),
    Heading3(Heading),
    Code(Code),
    Image(Image),
    Video(Video),
    File(FileBlock),
    PDF(PDF),
    ColumnList(ColumnList),
    Column(Column),
    Unsupported(String),

    // TODO: Implement
    Toggle,
    SyncedBlock,
    Template,
    Table,
    Bookmark,
    Divider,
    TableOfContents
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Heading {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub is_toggleable: bool
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PDF {
    pub pdf: File
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileBlock {
    pub file: File,
    pub caption: Vec<RichText>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Image {
    pub image: File
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Video {
    pub video: File
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ColumnList {
    pub children: Option<Vec<Column>>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Column {
    pub children: Option<Vec<Block>>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Callout {
    pub icon: Option<Icon>,
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Quote {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ToDoItem {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub checked: Option<bool>,
    pub children: Option<Vec<Block>>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ListItem {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Paragraph {
    pub color: Color,
    pub rich_text: Vec<RichText>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Code {
    pub language: CodeLanguage,
    pub caption: Vec<RichText>,
    pub rich_text: Vec<RichText>
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodeLanguage {
    #[serde(rename = "abap")]
    ABAP,
    Arduino,
    Bash,
    Basic,
    C,
    Clojure,
    #[serde(rename = "coffeescript")]
    CoffeeScript,
    #[serde(rename = "c++")]
    Cpp,
    #[serde(rename = "c#")]
    CSharp,
    #[serde(rename = "css")]
    CSS,
    Dart,
    Diff,
    Docker,
    Elixer,
    Elm,
    Erlang,
    Flow,
    Fortan,
    #[serde(rename = "f#")]
    FSharp,
    Gherkin,
    #[serde(rename = "glsl")]
    GLSL,
    Go,
    #[serde(rename = "graphql")]
    GraphQL,
    Groovy,
    Haskell,
    #[serde(rename = "html")]
    HTML,
    Java,
    #[serde(rename = "javascript")]
    JavaScript,
    #[serde(rename = "json")]
    JSON,
    Julia,
    Kotlin,
    Latex,
    Less,
    Lisp,
    #[serde(rename = "livescript")]
    LiveScript,
    Lua,
    #[serde(rename = "makefile")]
    MakeFile,
    Markdown,
    Markup,
    Matlab,
    Mermaid,
    Nix,
    #[serde(rename = "objective-c")]
    ObjectiveC,
    #[serde(rename = "ocaml")]
    OCaml,
    Pascal,
    Perl,
    #[serde(rename = "php")]
    PHP,
    #[default]
    #[serde(rename = "plain text")]
    PlainText,
    #[serde(rename = "powershell")]
    PowerShell,
    Prolog,
    Protobuf,
    Python,
    R,
    Reason,
    Ruby,
    Rust,
    Sass,
    Scala,
    Scheme,
    #[serde(rename = "scss")]
    SCSS,
    Shell,
    #[serde(rename = "sql")]
    SQL,
    Swift,
    #[serde(rename = "typescript")]
    TypeScript,
    #[serde(rename = "vb.net")]
    VBNet,
    Verilog,
    #[serde(rename = "vhdl")]
    VHDL,
    #[serde(rename = "visual basic")]
    VisualBasic,
    #[serde(rename = "webassembly")]
    WebAssembly,
    #[serde(rename = "xml")]
    XML,
    #[serde(rename = "yaml")]
    YAML,
    #[serde(rename = "java/c/c++/c#")]
    JavaCCppCSharp
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChildPage {
    pub title: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChildDatabase {
    pub title: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub id: String,
    pub name: String,
    pub person: Option<Person>,
    pub avatar_url: Option<String>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Workspace {
    pub workspace: bool
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Person {
    pub email: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Database {
    pub id: String,
    pub title: Vec<RichText>,
    pub description: Vec<RichText>,
    pub properties: Properties,
    pub url: String,

    pub parent: Parent,
    pub created_time: DateValue,
    pub last_edited_time: DateValue,
    pub last_edited_by: PartialUser,
    pub icon: Option<Icon>,
    pub cover: Option<File>,
    pub archived: bool,
    pub is_inline: bool
}

// TODO: Paginate all possible responses
#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
pub struct QueryResponse<T> {
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub results: Vec<T>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
pub struct Page {
    pub id: String,
    pub created_by: PartialUser,
    pub url: String,
    pub parent: Parent,

    pub created_time: DateValue,
    pub last_edited_time: DateValue,

    pub cover: Option<File>,
    pub icon: Option<Icon>,

    pub properties: Properties,

    pub archived: bool
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PartialUser {
    pub id: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub struct Properties {
    pub map: HashMap<String, Property>
}

impl Properties {
    pub fn get(&self, key: &str) -> Option<Property> {
        match self.map.get(key) {
            Some(property) => Some(property.to_owned()),
            None => None
        }
    }

    pub fn keys(&self) -> Vec<String> {
        self.map.keys()
            .map(|key| key.to_string())
            .collect()
    }
}

impl TryFrom<Value> for Properties {
    type Error = Error;

    fn try_from(data: Value) -> Result<Properties> {
        let mut map = HashMap::new();
        
        for key in data.as_object().unwrap().keys() {
            map.insert(key.to_owned(), parse(key, &data)?);
        }

        Ok(Properties { map })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum PropertyResponse {
    List(Vec<Property>),
    PropertyItem(Box<Property>)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
pub struct PartialProperty {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
pub enum PropertyType {
    RichText(Vec<RichText>),
    Number,
    Select(Select),
    MultiSelect(MultiSelect),
    Date(Date),
    Formula(Formula),
    Relation,
    Rollup,
    Title(Vec<RichText>),
    People,
    Files,
    Checkbox(bool),
    Url,
    Email,
    PhoneNumber,
    CreatedTime,
    CreatedBy,
    LastEditedTime,
    LastEditedBy,
    Unknown
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub enum Formula {
    String(Option<String>),
    Number(Option<i32>),
    Boolean(Option<bool>),
    Date(Option<Date>),
    Unknown
}

impl TryFrom<Value> for Formula {
    type Error = Error;

    fn try_from(data: Value) -> Result<Formula> {
        Ok(
            match parse::<String>("type", &data)?.as_str() {
                "string" => Formula::String(parse("string", &data)?),
                "number" => Formula::Number(parse("number", &data)?),
                "boolean" => Formula::Boolean(parse("boolean", &data)?),
                "date" => Formula::Date(parse("date", &data)?),
                _ => Formula::Unknown
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub struct Select(pub SelectOption);

impl TryFrom<Value> for Select {
    type Error = Error;

    fn try_from(data: Value) -> Result<Select> {
        Ok(Select(serde_json::from_value(data)?))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub struct MultiSelect(pub Vec<SelectOption>);

impl TryFrom<Value> for MultiSelect {
    type Error = Error;

    fn try_from(data: Value) -> Result<MultiSelect> {
        let options: Vec<SelectOption> = match data {
            Value::Array(_) => serde_json::from_value(data)?,
            Value::Object(_) => parse::<Vec<SelectOption>>("options", &data)?,
            _ => vec![]
        };

        Ok(MultiSelect(options))
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SelectOption {
    pub id: String,
    pub name: String,
    pub color: Color
}

impl std::fmt::Display for SelectOption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        Ok(write!(formatter, "MultiSelectOption::{}", self.name)?)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub enum RichText {
    Text(Text, String),
    Mention(Mention, String),
    Equation(Equation, String),
    Unknown
}

impl TryFrom<Value> for RichText {
    type Error = Error;

    fn try_from(data: Value) -> Result<RichText> {
        let plain_text = match &data.get("plain_text") {
            Some(Value::String(string)) => string.to_owned(),
            _ => "".to_string()
        };

        Ok(
            match parse::<String>("type", &data)?.as_str() {
                "text" => RichText::Text(serde_json::from_value(data)?, plain_text),
                "mention" => RichText::Mention(serde_json::from_value(data)?, plain_text),
                "equation" => RichText::Equation(serde_json::from_value(data)?, plain_text),
                _ => RichText::Unknown
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub struct Text {
    pub content: String,
    pub link: Option<Link>,

    pub plain_text: String,
    pub href: Option<String>,
    pub annotations: Annotations
}

impl TryFrom<Value> for Text {
    type Error = Error;
    
    fn try_from(data: Value) -> Result<Text> {
        let text = data.get("text")
            .ok_or_else(|| Error::NoSuchProperty("text".to_string()))?;

        Ok(
            Text {
                content: parse::<String>("content", text)?,
                link: if let Some(Value::String(_)) = text.get("link") {
                    Some(parse("link", text)?)
                } else {
                    None
                },

                plain_text: parse::<String>("plain_text", &data)?,
                href: if let Some(Value::String(_)) = text.get("href") {
                    Some(parse("href", &data)?)
                } else {
                    None
                },
                annotations: parse("annotations", &data)?
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub enum Mention {
    User(User),
    Page(PartialPage),
    Database(PartialDatabase),
    Date(Date),
    LinkPreview(LinkPreview),
    Unknown
}

impl TryFrom<Value> for Mention {
    type Error = Error;

    fn try_from(data: Value) -> Result<Mention> {
        let mention = data.get("mention")
            .ok_or_else(|| Error::NoSuchProperty("mention".to_string()))?;

        Ok(
            match parse::<String>("type", mention)?.as_str() {
                "user" => Mention::User(parse("user", mention)?),
                "page" => Mention::Page(parse("page", mention)?),
                "date" => Mention::Date(parse("date", mention)?),
                "database" => Mention::Database(parse("database", mention)?),
                "link_preview" => Mention::LinkPreview(parse("link_preview", mention)?),
                _ => Mention::Unknown
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LinkPreview {
    pub url: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PartialPage {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PartialDatabase {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PartialBlock {
    pub id: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Date {
    pub start: DateValue,
    pub end: Option<DateValue>
    // TODO: Implement for setting
    // pub time_zone: Option<String>
}

impl std::fmt::Display for Date {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        let start = &self.start;
        if let Some(end) = &self.end {
            Ok(write!(formatter, "{start} - {end}")?)
        } else {
            Ok(write!(formatter, "{start}")?)
        }
    }
}


#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "String", into = "String")]
pub enum DateValue {
    DateTime(DateTime<Utc>),
    Date(chrono::NaiveDate)
}

impl TryFrom<String> for DateValue {
    type Error = Error;

    fn try_from(string: String) -> Result<DateValue> {
        // NOTE: is either ISO 8601 Date or assumed to be ISO 8601 DateTime
        let value = if ISO_8601_DATE.is_match(&string) {
            DateValue::Date(
                DateTime::parse_from_rfc3339(&format!("{string}T00:00:00Z"))?
                    .date_naive()
            )
        } else {
            DateValue::DateTime(
                DateTime::parse_from_rfc3339(&string)?
                    .with_timezone(&Utc)
            )
        };

        Ok(value)
    }
}

impl From<DateValue> for String {
    fn from(value: DateValue) -> String {
        value.to_string()
    }
}

impl std::fmt::Display for DateValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        let value = match self {
            DateValue::Date(date) => DateTime::<Utc>::from_utc(
                date.and_hms_opt(0, 0, 0)
                    .expect("to parse NaiveDate into DateTime "), 
                Utc
            ).to_rfc3339(),
            DateValue::DateTime(date_time) => date_time.to_rfc3339()
        };
        Ok(write!(formatter, "{}", value)?)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Equation {
    pub plain_text: String
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Link {
    pub url: String
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Annotations {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub code: bool,
    pub color: Color
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    #[default]
    Default,
    Gray,
    Brown,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Pink,
    Red,

    GrayBackground,
    BrownBackground,
    OrangeBackground,
    YellowBackground,
    GreenBackground,
    BlueBackground,
    PurpleBackground,
    PinkBackground,
    RedBackground
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
#[serde(try_from = "Value")]
// FIXME: Convert to enum / PropertyType
pub struct Property {
    pub id: String,
    pub next_url: Option<String>,
    pub value: PropertyType
}

impl TryFrom<Value> for Property {
    type Error = Error;

    fn try_from(data: Value) -> Result<Property> {
        Ok(
            Property {
                id: data.get("id").ok_or_else(|| Error::NoSuchProperty("id".to_string()))?.to_string(),
                value: match parse::<String>("type", &data)?.as_str() {
                    "title" => PropertyType::Title(parse("title", &data)?),
                    "rich_text" => PropertyType::RichText(parse("rich_text", &data)?),
                    "date" => PropertyType::Date(parse("date", &data)?),
                    "multi_select" => PropertyType::MultiSelect(parse("multi_select", &data)?),
                    "select" => PropertyType::Select(parse("select", &data)?),
                    "formula" => PropertyType::Formula(parse("formula", &data)?),
                    "checkbox" => PropertyType::Checkbox(parse("checkbox", &data)?),
                    _ => PropertyType::Unknown
                },
                next_url: match data.get("next_url") {
                    Some(value) => Some(value.as_str().ok_or_else(|| Error::NoSuchProperty("next_url".to_string()))?.to_string()),
                    None => None
                }
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub enum Parent {
    Page(PartialPage),
    Database(PartialDatabase),
    Block(PartialBlock),
    Workspace(bool),
    Unknown
}

impl TryFrom<Value> for Parent {
    type Error = Error;

    fn try_from(data: Value) -> Result<Parent> {
        Ok(
            match parse::<String>("type", &data)?.as_str() {
                "page_id" => Parent::Page(PartialPage { id: parse(parse::<String>("type", &data)?.as_str(), &data)? }),
                "database_id" => Parent::Database(PartialDatabase { id: parse(parse::<String>("type", &data)?.as_str(), &data)? }),
                "block_id" => Parent::Block(PartialBlock { id: parse(parse::<String>("type", &data)?.as_str(), &data)? }),
                "workspace" => Parent::Workspace(parse("workspace", &data)?),
                _ => Parent::Unknown
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
#[allow(unused)]
pub enum File {
    Notion(String, DateValue),
    External(String),
    Unknown
}

impl TryFrom<Value> for File {
    type Error = Error;

    fn try_from(data: Value) -> Result<File> {
        Ok(
            match parse::<String>("type", &data)?.as_str() {
                "file" => {
                    let file = data.get("file").ok_or_else(|| Error::NoSuchProperty("file".to_string()))?;
                    File::Notion(
                        parse::<String>("url", file)?,
                        parse::<DateValue>("expiry_time", file)?
                    )
                },
                "external" => {
                    let external = data.get("external").ok_or_else(|| Error::NoSuchProperty("file".to_string()))?;
                    File::External(parse::<String>("url", external)?)
                },
                _ => File::Unknown
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(try_from = "Value")]
pub enum Icon {
    File(File),
    Emoji(String),
    Unknown
}

impl TryFrom<Value> for Icon {
    type Error = Error;

    fn try_from(data: Value) -> Result<Icon> {
        Ok(
            match parse::<String>("type", &data)?.as_str() {
                "file" => Icon::File(serde_json::from_value::<File>(data)?),
                "emoji" => Icon::Emoji(parse::<String>("emoji", &data)?),
                _ => Icon::Unknown
            }
        )
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        Ok(write!(formatter, "NotionError::{:?}", self)?)
    }
}


