use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use lazy_static::lazy_static;
use regex::Regex;
#[cfg(feature = "request")]
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;

use futures_core::future::BoxFuture;

lazy_static! {
    static ref ISO_8601_DATE: Regex =
        Regex::new(r"^\d{4}-\d{2}-\d{2}$").expect("ISO 8601 date regex to be parseable");
}

#[cfg(feature = "request")]
const NOTION_VERSION: &str = "2022-06-28";

pub type Result<T> = std::result::Result<T, Error>;
pub type Callback = dyn Fn(
        &mut reqwest::RequestBuilder,
    ) -> BoxFuture<'_, std::result::Result<reqwest::Response, reqwest::Error>>
    + 'static
    + Send
    + Sync;

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error, Option<Value>),
    Deserialization(serde_json::Error, Option<Value>),
    Header(reqwest::header::InvalidHeaderValue),
    ChronoParse(chrono::ParseError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        Ok(write!(formatter, "NotionError::{:?}", self)?)
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Http(error, None)
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

async fn try_to_parse_response<T: std::fmt::Debug + for<'de> serde::Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T> {
    let text = response.text().await?;

    match serde_json::from_str::<T>(&text) {
        Ok(value) => Ok(value),
        Err(error) => match serde_json::from_str::<Value>(&text) {
            Ok(body) => {
                println!("Error: {error:#?}\n\nBody: {body:#?}");

                Err(Error::Deserialization(error, None))
            }
            _ => {
                println!("Error: {error:#?}\n\nBody: {text}");

                Err(Error::Deserialization(error, None))
            }
        },
    }
}

#[cfg(feature = "request")]
fn get_http_client(notion_api_key: &str) -> reqwest::Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {notion_api_key}"))
            .expect("bearer token to be parsed into a header"),
    );
    headers.insert(
        "Notion-Version",
        HeaderValue::from_str(NOTION_VERSION).expect("notion version to be parsed into a header"),
    );
    headers.insert("Content-Type", HeaderValue::from_static("application/json"));

    reqwest::ClientBuilder::new()
        .default_headers(headers)
        .build()
        .expect("to build a valid client out of notion_api_key")
}

#[derive(Serialize)]
pub struct SearchOptions<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_cursor: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
}

#[derive(Default)]
pub struct ClientBuilder {
    api_key: Option<String>,
    custom_request: Option<Arc<Callback>>,
}

impl ClientBuilder {
    pub fn api_key(mut self, api_key: &str) -> Self {
        self.api_key = Some(api_key.to_owned());

        self
    }

    pub fn custom_request<F>(mut self, callback: F) -> Self
    where
        for<'c> F: Fn(
                &'c mut reqwest::RequestBuilder,
            ) -> BoxFuture<'c, std::result::Result<reqwest::Response, reqwest::Error>>
            + 'static
            + Send
            + Sync,
    {
        self.custom_request = Some(Arc::new(callback));

        self
    }

    #[cfg(feature = "request")]
    pub fn build(self) -> Client {
        let notion_api_key = self.api_key.expect("api_key to be set");

        let request_handler = self.custom_request.unwrap_or(Arc::new(
            |request_builder: &mut reqwest::RequestBuilder| {
                Box::pin(async move {
                    let request = request_builder
                        .try_clone()
                        .expect("non-stream body request clone to succeed");

                    request.send().await
                })
            },
        ));

        let http_client = Arc::from(get_http_client(&notion_api_key));

        Client {
            http_client: http_client.clone(),
            request_handler: request_handler.clone(),

            pages: Pages {
                http_client: http_client.clone(),
                request_handler: request_handler.clone(),
            },
            blocks: Blocks {
                http_client: http_client.clone(),
                request_handler: request_handler.clone(),
            },
            databases: Databases {
                http_client: http_client.clone(),
                request_handler: request_handler.clone(),
            },
            users: Users {
                http_client: http_client.clone(),
                request_handler: request_handler.clone(),
            },
        }
    }
}

pub struct Client {
    http_client: Arc<reqwest::Client>,
    request_handler: Arc<Callback>,

    pub pages: Pages,
    pub blocks: Blocks,
    pub databases: Databases,
    pub users: Users,
}

impl<'a> Client {
    pub fn new() -> ClientBuilder {
        ClientBuilder::default()
    }

    pub async fn search<'b, T: std::fmt::Debug + for<'de> serde::Deserialize<'de>>(
        self,
        options: SearchOptions<'b>,
    ) -> Result<QueryResponse<T>> {
        let mut request = self
            .http_client
            .post("https://api.notion.com/v1/search")
            .json(&options);

        let response = (self.request_handler)(&mut request).await?;

        match response.error_for_status_ref() {
            Ok(_) => Ok(response.json().await?),
            Err(error) => {
                let body = response.json::<Value>().await?;
                Err(Error::Http(error, Some(body)))
            }
        }
    }
}

pub struct PageOptions<'a> {
    pub page_id: &'a str,
}

#[derive(Clone)]
pub struct Pages {
    http_client: Arc<reqwest::Client>,
    request_handler: Arc<Callback>,
}

impl Pages {
    pub async fn retrieve<'a>(self, options: PageOptions<'a>) -> Result<Page> {
        let url = format!(
            "https://api.notion.com/v1/pages/{page_id}",
            page_id = options.page_id
        );

        let mut request = self.http_client.get(url);

        let response = (self.request_handler)(&mut request).await?;

        match response.error_for_status_ref() {
            Ok(_) => Ok(response.json().await?),
            Err(error) => {
                let body = response.json::<Value>().await?;
                Err(Error::Http(error, Some(body)))
            }
        }
    }
}

#[derive(Clone)]
pub struct Blocks {
    http_client: Arc<reqwest::Client>,
    request_handler: Arc<Callback>,
}

impl Blocks {
    pub fn children(&self) -> BlockChildren {
        BlockChildren {
            http_client: self.http_client.clone(),
            request_handler: self.request_handler.clone(),
        }
    }
}

pub struct BlockChildren {
    http_client: Arc<reqwest::Client>,
    request_handler: Arc<Callback>,
}

pub struct BlockChildrenListOptions<'a> {
    pub block_id: &'a str,
}

impl BlockChildren {
    pub async fn list<'a>(
        self,
        options: BlockChildrenListOptions<'a>,
    ) -> Result<QueryResponse<Block>> {
        let url = format!(
            "https://api.notion.com/v1/blocks/{block_id}/children",
            block_id = options.block_id
        );

        let mut request = self.http_client.get(&url);

        let response = (self.request_handler)(&mut request).await?;

        match response.error_for_status_ref() {
            Ok(_) => Ok(response.json().await?),
            Err(error) => {
                let body = response.json::<Value>().await?;
                Err(Error::Http(error, Some(body)))
            }
        }
    }
}

#[derive(Clone)]
pub struct Databases {
    http_client: Arc<reqwest::Client>,
    request_handler: Arc<Callback>,
}

impl Databases {
    pub async fn query<'a>(
        &self,
        options: DatabaseQueryOptions<'a>,
    ) -> Result<QueryResponse<Page>> {
        let url = format!(
            "https://api.notion.com/v1/databases/{database_id}/query",
            database_id = options.database_id
        );

        let mut request = self.http_client.post(url);

        let json = if let Some(filter) = options.filter {
            Some(json!({ "filter": filter }))
        } else {
            None
        };

        let json = if let Some(sorts) = options.sorts {
            if let Some(mut json) = json {
                json.as_object_mut()
                    .expect("Some object to be editable")
                    .insert("sorts".to_string(), sorts);

                Some(json)
            } else {
                Some(json!({ "sorts": sorts }))
            }
        } else {
            json
        };

        let json = if let Some(cursor) = options.start_cursor {
            if let Some(mut json) = json {
                json.as_object_mut()
                    .expect("Some object to be editable")
                    .insert("start_cursor".to_string(), Value::String(cursor));

                Some(json)
            } else {
                Some(json!({ "start_cursor": cursor }))
            }
        } else {
            json
        };

        if let Some(json) = json {
            request = request.json(&json);
        }

        let response = (self.request_handler)(&mut request).await?;

        match response.error_for_status_ref() {
            Ok(_) => try_to_parse_response(response).await,
            Err(error) => {
                let body = try_to_parse_response::<Value>(response).await?;
                Err(Error::Http(error, Some(body)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn check_database_query() {
        let databases = Client::new()
            .api_key("secret_FuhJkAoOVZlk8YUT9ZOeYqWBRRZN6OMISJwhb4dTnud")
            .build()
            .search::<Database>(SearchOptions {
                filter: Some(json!(
                    {
                        "value": "database",
                        "property": "object"
                    }
                )),
                query: None,
                page_size: None,
                sort: None,
                start_cursor: None,
            })
            .await;

        println!("{databases:#?}");
    }

    #[tokio::test]
    async fn test_blocks() {
        let blocks = Client::new()
            .api_key("secret_FuhJkAoOVZlk8YUT9ZOeYqWBRRZN6OMISJwhb4dTnud")
            .build()
            .blocks
            .children()
            .list(BlockChildrenListOptions {
                block_id: "0d253ab0f751443aafb9bcec14012897",
            })
            .await;

        println!("{blocks:#?}")
    }
}

#[derive(Debug, Default)]
pub struct DatabaseQueryOptions<'a> {
    pub database_id: &'a str,
    // TODO: Implement spec for filter?
    pub filter: Option<Value>,
    pub sorts: Option<Value>,
    pub start_cursor: Option<String>,
}

#[derive(Clone)]
pub struct Users {
    http_client: Arc<reqwest::Client>,
    request_handler: Arc<Callback>,
}

impl Users {
    pub async fn get(&self) -> Result<QueryResponse<User>> {
        let url = "https://api.notion.com/v1/users".to_owned();

        let mut request = self.http_client.get(&url);

        let response = (self.request_handler)(&mut request).await?;

        match response.error_for_status_ref() {
            Ok(_) => Ok(response.json().await?),
            Err(error) => {
                let body = response.json::<Value>().await?;
                Err(Error::Http(error, Some(body)))
            }
        }
    }
}

// Start of normal entities

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Block {
    pub id: String,
    pub parent: Parent,
    pub created_time: DateValue,
    pub last_edited_time: DateValue,
    pub created_by: PartialUser,
    pub last_edited_by: PartialUser,
    pub has_children: bool,
    pub archived: bool,
    #[serde(flatten)]
    pub block: BlockType,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    Paragraph {
        paragraph: Paragraph,
    },
    Bookmark {
        bookmark: Bookmark,
    },
    Breadcrumb,
    BulletedListItem {
        bulleted_list_item: ListItem,
    },
    Callout {
        callout: Callout,
    },
    ChildDatabase,
    ChildPage,
    Code {
        code: Code,
    },
    Column,
    ColumnList,
    Divider,
    Embed {
        embed: Embed,
    },
    Equation {
        equation: Equation,
    },
    File {
        file: File,
    },
    Heading1 {
        heading: Heading,
    },
    Heading2 {
        heading: Heading,
    },
    Heading3 {
        heading: Heading,
    },
    Image {
        image: File,
    },
    LinkPreview {
        link_preview: LinkPreview,
    },
    LinkToPage,
    NumberedListItem {
        numbered_list_item: ListItem,
    },
    Pdf {
        pdf: File,
    },
    Quote {
        quote: Quote,
    },
    SyncedBlock,
    Table,
    TableOfContents,
    TableRow,
    Template,
    ToDo {
        to_do: ToDoItem,
    },
    Toggle,
    Video {
        video: File,
    },

    // TODO: Implement Unsupported(Value)
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Embed {
    url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Bookmark {
    caption: Vec<RichText>,
    url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Heading {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub is_toggleable: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PDF {
    pub pdf: File,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct FileBlock {
    pub file: File,
    pub caption: Vec<RichText>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ColumnList {
    pub children: Option<Vec<Column>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Column {
    pub children: Option<Vec<Block>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Callout {
    pub icon: Option<Icon>,
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Quote {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ToDoItem {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub checked: Option<bool>,
    pub children: Option<Vec<Block>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ListItem {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Paragraph {
    pub color: Color,
    pub rich_text: Vec<RichText>,
    pub children: Option<Vec<Block>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct Code {
    caption: Vec<RichText>,
    rich_text: Vec<RichText>,
    language: CodeLanguage,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
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
    JavaCCppCSharp,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChildPage {
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ChildDatabase {
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct User {
    pub id: String,
    pub name: Option<String>,
    pub person: Option<Person>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Workspace {
    pub workspace: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Person {
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Database {
    pub id: String,
    pub title: Vec<RichText>,
    pub description: Vec<RichText>,
    pub properties: HashMap<String, DatabaseProperty>,
    pub url: String,

    pub parent: Parent,
    pub created_time: DateValue,
    pub last_edited_time: DateValue,
    pub last_edited_by: PartialUser,
    pub icon: Option<Icon>,
    pub cover: Option<File>,
    pub archived: bool,
    pub is_inline: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub struct DatabaseSelectOptions {
    pub options: Vec<SelectOption>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum DatabaseProperty {
    Checkbox {
        id: String,
        name: String,
    },
    CreatedTime {
        id: String,
        name: String,
    },
    Date {
        id: String,
        name: String,
    },
    Email {
        id: String,
        name: String,
    },
    Files {
        id: String,
        name: String,
    },
    Formula {
        id: String,
        name: String,
        formula: DatabaseFormula,
    },
    LastEditedBy {
        id: String,
        name: String,
    },
    LastEditedTime {
        id: String,
        name: String,
    },
    MultiSelect {
        id: String,
        name: String,
        multi_select: DatabaseSelectOptions,
    },
    Number {
        id: String,
        name: String,
        number: Number,
    },
    People {
        id: String,
        name: String,
    },
    PhoneNumber {
        id: String,
        name: String,
    },
    Relation {
        id: String,
        name: String,
        // relation: Relation,
    },
    RichText {
        id: String,
        name: String,
    },
    Rollup {
        id: String,
        name: String,
        // TODO: Implement Rollup
    },
    Select {
        id: String,
        name: String,
        select: DatabaseSelectOptions,
    },
    Status {
        id: String,
        name: String,
        // TODO: Implement Status
    },
    Title {
        id: String,
        name: String,
    },
    Url {
        id: String,
        name: String,
    },

    // TODO: Implement Unsupported(Value)
    #[serde(other)]
    Unsupported,
}

impl DatabaseProperty {
    pub fn id(&self) -> Option<String> {
        use DatabaseProperty::*;

        match self {
            Checkbox { id, .. }
            | CreatedTime { id, .. }
            | Date { id, .. }
            | Email { id, .. }
            | Files { id, .. }
            | Formula { id, .. }
            | LastEditedBy { id, .. }
            | LastEditedTime { id, .. }
            | MultiSelect { id, .. }
            | Number { id, .. }
            | People { id, .. }
            | PhoneNumber { id, .. }
            | Relation { id, .. }
            | RichText { id, .. }
            | Rollup { id, .. }
            | Select { id, .. }
            | Status { id, .. }
            | Title { id, .. }
            | Url { id, .. } => Some(id.to_owned()),

            Unsupported => None,
        }
    }

    pub fn name(&self) -> Option<String> {
        use DatabaseProperty::*;

        match self {
            Checkbox { name, .. }
            | CreatedTime { name, .. }
            | Date { name, .. }
            | Email { name, .. }
            | Files { name, .. }
            | Formula { name, .. }
            | LastEditedBy { name, .. }
            | LastEditedTime { name, .. }
            | MultiSelect { name, .. }
            | Number { name, .. }
            | People { name, .. }
            | PhoneNumber { name, .. }
            | Relation { name, .. }
            | RichText { name, .. }
            | Rollup { name, .. }
            | Select { name, .. }
            | Status { name, .. }
            | Title { name, .. }
            | Url { name, .. } => Some(name.to_owned()),

            Unsupported => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Number {
    // TODO: Implement NumberFormat
    // pub format: NumberFormat
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Relation {
    // #[serde(alias = "database_id")]
    // id: String,
    // synced_property_name: String,
    // synced_property_id: String,
}

// TODO: Paginate all possible responses
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct QueryResponse<T> {
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub results: Vec<T>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Page {
    pub id: String,
    pub created_by: PartialUser,
    pub url: String,
    pub parent: Parent,

    pub created_time: DateValue,
    pub last_edited_time: DateValue,

    pub cover: Option<File>,
    pub icon: Option<Icon>,

    pub properties: HashMap<String, Property>,

    pub archived: bool,
}

impl Page {
    pub fn get_property_by_id(&self, id: &str) -> Option<(&String, &Property)> {
        self.properties.iter().find(|(_, property)| {
            property.id().is_some()
                && property.id().expect("id that is_some() to be unwrappable") == id
        })
    }

    pub fn get_title(&self) -> &Vec<RichText> {
        if let Property::Title { title, .. } = self
            .get_property_by_id("title")
            .expect("every page to have a title")
            .1
        {
            title
        } else {
            unreachable!("Expected title to be of type title")
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum Property {
    Checkbox {
        id: String,
        checkbox: bool,
    },
    CreatedBy {
        id: String,
    },
    CreatedTime {
        id: String,
        created_time: DateValue,
    },
    Date {
        id: String,
        date: Option<Date>,
    },
    Email {
        id: String,
        email: Option<String>,
    },
    Files {
        id: String,
        files: Vec<File>,
    },
    Formula {
        id: String,
        formula: Formula,
    },
    LastEditedBy {
        id: String,
    }, // TODO: Implement LastEditedBy
    LastEditedTime {
        id: String,
        last_edited_time: DateValue,
    },
    Select {
        id: String,
        select: Option<SelectOption>,
    },
    MultiSelect {
        id: String,
        multi_select: Vec<SelectOption>,
    },
    Number {
        id: String,
        number: Option<f32>,
    },
    People {
        id: String,
    },
    PhoneNumber {
        id: String,
    },
    Relation {
        id: String,
        relation: Vec<Relation>,
    },
    Rollup {
        id: String,
    }, // TODO: Implement Rollup
    RichText {
        id: String,
        rich_text: Vec<RichText>,
    },
    Status {
        id: String,
    }, // TODO: Implement Status
    Title {
        id: String,
        title: Vec<RichText>,
    },
    Url {
        id: String,
        url: Option<String>,
    },

    // TODO: Implement Unsupported(Value)
    #[serde(other)]
    Unsupported,
}

impl Property {
    pub fn id(&self) -> Option<String> {
        use Property::*;

        match self {
            Title { id, .. }
            | Checkbox { id, .. }
            | CreatedBy { id, .. }
            | CreatedTime { id, .. }
            | Date { id, .. }
            | Email { id, .. }
            | Files { id, .. }
            | LastEditedBy { id, .. }
            | MultiSelect { id, .. }
            | Number { id, .. }
            | People { id, .. }
            | LastEditedTime { id, .. }
            | PhoneNumber { id, .. }
            | Relation { id, .. }
            | Rollup { id, .. }
            | RichText { id, .. }
            | Select { id, .. }
            | Status { id, .. }
            | Url { id, .. }
            | Formula { id, .. } => Some(id.to_owned()),

            Unsupported => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum Formula {
    Boolean { boolean: Option<bool> },
    Date { date: Option<Date> },
    Number { number: Option<f32> },
    String { string: Option<String> },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PartialUser {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PartialProperty {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct DatabaseFormula {
    pub expression: String,
    pub suspected_type: Option<DatabaseFormulaType>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SelectOption {
    pub id: String,
    pub name: String,
    pub color: Color,
}

impl std::fmt::Display for SelectOption {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        Ok(write!(formatter, "MultiSelectOption::{}", self.name)?)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum RichText {
    Text {
        text: Text,
        plain_text: String,
        href: Option<String>,
        annotations: Annotations,
    },
    Mention {
        mention: Mention,
        plain_text: String,
        href: Option<String>,
        annotations: Annotations,
    },
    Equation {
        expression: Option<String>,
        plain_text: String,
        href: Option<String>,
        annotations: Annotations,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Text {
    pub content: String,
    pub link: Option<Link>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum Mention {
    Database { database: PartialDatabase },
    Date { date: Date },
    LinkPreview { link_preview: LinkPreview },
    Page { page: PartialPage },
    User { user: PartialUser },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct LinkPreview {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PartialPage {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PartialDatabase {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PartialBlock {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Date {
    pub start: DateValue,
    pub end: Option<DateValue>,
    // TODO: Implement for setting
    pub time_zone: Option<String>,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(try_from = "String", into = "String")]
pub enum DateValue {
    DateTime(DateTime<Utc>),
    Date(chrono::NaiveDate),
}

impl TryFrom<String> for DateValue {
    type Error = Error;

    fn try_from(string: String) -> Result<DateValue> {
        // NOTE: is either ISO 8601 Date or assumed to be ISO 8601 DateTime
        let value = if ISO_8601_DATE.is_match(&string) {
            DateValue::Date(
                DateTime::parse_from_rfc3339(&format!("{string}T00:00:00Z"))?.date_naive(),
            )
        } else {
            DateValue::DateTime(DateTime::parse_from_rfc3339(&string)?.with_timezone(&Utc))
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
                Utc,
            )
            .to_rfc3339(),
            DateValue::DateTime(date_time) => date_time.to_rfc3339(),
        };
        Ok(write!(formatter, "{}", value)?)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Equation {
    pub plain_text: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Link {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct Annotations {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    pub code: bool,
    pub color: Color,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
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
    RedBackground,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum Parent {
    PageId { page_id: String },
    DatabaseId { database_id: String },
    BlockId { block_id: String },
    Workspace,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum File {
    File { file: NotionFile },
    External { external: ExternalFile },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum Icon {
    Emoji { emoji: String },
    File { file: NotionFile },
    External { external: ExternalFile },
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct NotionFile {
    expiry_time: DateValue,
    url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ExternalFile {
    url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseFormulaType {
    Boolean,
    Date,
    Number,
    String,
}
