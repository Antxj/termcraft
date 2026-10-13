/// Human/agent-facing metadata for a command. `input_schema` is JSON Schema
/// for the `args` object and is served verbatim as the MCP tool schema.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: &'static str,
    /// Valid example args; a unit test keeps the catalog in sync with `Command`.
    pub example_args: &'static str,
    /// Mutates persistent state or the running app.
    pub mutating: bool,
}

const EMPTY: &str = r#"{"type":"object","properties":{},"additionalProperties":false}"#;

pub const CATALOG: &[CommandSpec] = &[
    CommandSpec {
        name: "app_version",
        title: "App version",
        description: "Return the TermCraft version.",
        input_schema: EMPTY,
        example_args: "{}",
        mutating: false,
    },
    CommandSpec {
        name: "session_list",
        title: "List sessions",
        description: "List all saved sessions with their connection settings.",
        input_schema: EMPTY,
        example_args: "{}",
        mutating: false,
    },
    CommandSpec {
        name: "session_add",
        title: "Add session",
        description: "Save a new session. `session.type` is one of local_shell, ssh, telnet, \
                      serial, rdp, vnc, sftp, ftp; remaining fields depend on the type \
                      (e.g. ssh: host, port, user, private_key, x11_forwarding).",
        input_schema: r#"{"type":"object","required":["session"],"properties":{"session":{"type":"object","required":["name","type"],"properties":{"name":{"type":"string"},"folder":{"type":"string"},"type":{"type":"string","enum":["local_shell","ssh","telnet","serial","rdp","vnc","sftp","ftp"]},"host":{"type":"string"},"port":{"type":"integer"},"user":{"type":"string"}}}}}"#,
        example_args: r#"{"session":{"name":"web","type":"ssh","host":"10.0.0.1","user":"root"}}"#,
        mutating: true,
    },
    CommandSpec {
        name: "session_remove",
        title: "Remove session",
        description: "Delete a saved session by id.",
        input_schema: r#"{"type":"object","required":["id"],"properties":{"id":{"type":"string","format":"uuid"}}}"#,
        example_args: r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "session_import_mobaxterm",
        title: "Import MobaXterm sessions",
        description: "Import sessions from an exported .mxtsessions file or a MobaXterm.ini. \
                      Existing names are skipped; unsupported session types are reported.",
        input_schema: r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}"#,
        example_args: r#"{"path":"C:/Users/me/Desktop/sessions.mxtsessions"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "session_rename",
        title: "Rename session",
        description: "Rename a saved session (names are unique).",
        input_schema: r#"{"type":"object","required":["id","name"],"properties":{"id":{"type":"string","format":"uuid"},"name":{"type":"string"}}}"#,
        example_args: r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8","name":"web-02"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "session_duplicate",
        title: "Duplicate session",
        description: "Copy a saved session under a new unique name.",
        input_schema: r#"{"type":"object","required":["id"],"properties":{"id":{"type":"string","format":"uuid"}}}"#,
        example_args: r#"{"id":"67e55044-10b1-426f-9247-bb680e5fe0c8"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "tab_list",
        title: "List tabs",
        description: "List open tabs in the running app.",
        input_schema: EMPTY,
        example_args: "{}",
        mutating: false,
    },
    CommandSpec {
        name: "tab_open",
        title: "Open tab",
        description: "Open a new tab for a saved session (by name or id), or a local shell if omitted.",
        input_schema: r#"{"type":"object","properties":{"session":{"type":"string"}}}"#,
        example_args: r#"{"session":"web"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "tab_send",
        title: "Send text to tab",
        description: "Type text into a tab (the active tab if `tab` is omitted). Include \\r to press Enter.",
        input_schema: r#"{"type":"object","required":["text"],"properties":{"tab":{"type":"integer"},"text":{"type":"string"}}}"#,
        example_args: r#"{"text":"uptime\r"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "tab_quick_connect",
        title: "Quick connect",
        description: "Open a saved session by name or host, otherwise an ad-hoc SSH tab for `[user@]host[:port]`.",
        input_schema: r#"{"type":"object","required":["target"],"properties":{"target":{"type":"string"}}}"#,
        example_args: r#"{"target":"root@10.0.0.1:22"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "tab_close",
        title: "Close tab",
        description: "Close a tab (the active tab if `tab` is omitted).",
        input_schema: r#"{"type":"object","properties":{"tab":{"type":"integer"}}}"#,
        example_args: "{}",
        mutating: true,
    },
    CommandSpec {
        name: "tab_duplicate",
        title: "Duplicate tab",
        description: "Open the same session as a tab (the active tab if omitted) in a new tab.",
        input_schema: r#"{"type":"object","properties":{"tab":{"type":"integer"}}}"#,
        example_args: r#"{"tab":1}"#,
        mutating: true,
    },
    CommandSpec {
        name: "tab_rename",
        title: "Rename tab",
        description: "Set a tab's title (the active tab if `tab` is omitted).",
        input_schema: r#"{"type":"object","required":["title"],"properties":{"tab":{"type":"integer"},"title":{"type":"string"}}}"#,
        example_args: r#"{"title":"prod db"}"#,
        mutating: true,
    },
    CommandSpec {
        name: "update_check",
        title: "Check for updates",
        description: "Check the update channel for a newer signed release.",
        input_schema: EMPTY,
        example_args: "{}",
        mutating: false,
    },
];

pub fn spec(name: &str) -> Option<&'static CommandSpec> {
    CATALOG.iter().find(|s| s.name == name)
}
