//! Settings → Diff & merge: layout, whitespace, context and the conflict tool,
//! with a live preview built from the same row shapes as the real diff pane.

use std::path::Path;
use std::process::{Command, Stdio};

use gpui::prelude::FluentBuilder;
use gpui::{
    div, px, AnyElement, App, Context, IntoElement, ParentElement, SharedString, Styled, Window,
};

use ui::metrics::{FONT_MONO, FS_SM, R_MD};
use ui::theme::ActiveTheme;
use ui::{Button, ButtonSize, IconName};

use super::form;
use super::{group, heading, SettingsView};
use crate::settings::{DiffLayout, MergeTool, WhitespaceMode, DIFF_CONTEXT_MAX};

const TOOLS: [MergeTool; 5] = [
    MergeTool::Builtin,
    MergeTool::VsCode,
    MergeTool::Vimdiff,
    MergeTool::Meld,
    MergeTool::Custom,
];

const WHITESPACE: [(WhitespaceMode, &str); 3] = [
    (WhitespaceMode::None, "Show all"),
    (WhitespaceMode::Eol, "Ignore at line end"),
    (WhitespaceMode::All, "Ignore all"),
];

pub(super) fn render(
    view: &mut SettingsView,
    _window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    let settings = view.settings(cx);
    let colors = cx.theme().colors;
    let mut page = div().flex().flex_col();

    page = page.child(heading(
        "Diff & merge",
        "How changes are displayed, and which tool resolves conflicts.",
    ));

    let layout = {
        let entity = cx.entity();
        form::segmented(
            "settings-diff-layout".into(),
            ["Unified", "Side-by-side"],
            usize::from(settings.diff_layout == DiffLayout::Split),
            false,
            move |index, _, cx| {
                let layout = if index == 1 {
                    DiffLayout::Split
                } else {
                    DiffLayout::Unified
                };
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_layout = layout);
                });
            },
        )
    };
    let word = {
        let entity = cx.entity();
        form::switch(
            "settings-diff-word".into(),
            settings.diff_word_highlight,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_word_highlight = checked);
                });
            },
        )
    };
    let wrap = {
        let entity = cx.entity();
        form::switch(
            "settings-diff-wrap".into(),
            settings.diff_wrap,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_wrap = checked);
                });
            },
        )
    };
    let numbers = {
        let entity = cx.entity();
        form::switch(
            "settings-diff-numbers".into(),
            settings.diff_line_numbers,
            false,
            move |checked, _, cx| {
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_line_numbers = checked);
                });
            },
        )
    };

    page = page.child(group(
        "Display",
        vec![
            form::row(
                "settings-diff-layout-row",
                "Layout",
                "Side-by-side needs a details pane at least 900 px wide; narrower panes fall back to unified.",
                layout,
                false,
            ),
            form::row(
                "settings-diff-word-row",
                "Highlight changed words",
                "Marks the exact characters that differ inside a modified line.",
                word,
                false,
            ),
            form::row(
                "settings-diff-wrap-row",
                "Wrap long lines",
                "Off scrolls horizontally.",
                wrap,
                false,
            ),
            form::row(
                "settings-diff-numbers-row",
                "Line numbers",
                "",
                numbers,
                false,
            ),
        ],
    ));

    let whitespace = {
        let entity = cx.entity();
        form::select(
            "settings-diff-whitespace".into(),
            WHITESPACE.iter().map(|(_, label)| *label),
            WHITESPACE
                .iter()
                .position(|(mode, _)| *mode == settings.diff_whitespace)
                .unwrap_or(0),
            false,
            move |index, _, cx| {
                let mode = WHITESPACE[index.min(WHITESPACE.len() - 1)].0;
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_whitespace = mode);
                });
            },
        )
    };
    let context = {
        let entity = cx.entity();
        let decrease = {
            let entity = entity.clone();
            move |_: &mut Window, cx: &mut App| {
                let value = entity.read(cx).settings(cx).diff_context.saturating_sub(1);
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_context = value);
                });
            }
        };
        let increase = {
            let entity = entity.clone();
            move |_: &mut Window, cx: &mut App| {
                let value = (entity.read(cx).settings(cx).diff_context + 1).min(DIFF_CONTEXT_MAX);
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| settings.diff_context = value);
                });
            }
        };
        form::stepper(
            "settings-diff-context",
            settings.diff_context.to_string(),
            settings.diff_context > 0,
            settings.diff_context < DIFF_CONTEXT_MAX,
            colors,
            decrease,
            increase,
        )
    };
    page = page.child(group(
        "Content",
        vec![
            form::row(
                "settings-diff-whitespace-row",
                "Whitespace",
                "Hidden whitespace-only changes are still staged and committed.",
                whitespace,
                false,
            ),
            form::row(
                "settings-diff-context-row",
                "Context lines",
                "Unchanged lines shown around each change (0–20).",
                context,
                false,
            ),
        ],
    ));

    let preview = diff_preview(&settings, cx);
    page = page.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .mb(px(20.))
            .max_w(px(760.))
            .child(
                div()
                    .text_size(ui::metrics::FS_XS)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("PREVIEW"),
            )
            .child(
                div()
                    .border_1()
                    .rounded(R_MD)
                    .overflow_hidden()
                    .bg(colors.bg)
                    .child(
                        div()
                            .px(px(12.))
                            .py(px(6.))
                            .border_b_1()
                            .border_color(colors.border)
                            .bg(colors.surface)
                            .font_family(FONT_MONO)
                            .text_size(FS_SM)
                            .text_color(colors.muted)
                            .child(format!(
                                "src/history/graph.rs · {}{}",
                                if settings.diff_layout == DiffLayout::Split {
                                    "side-by-side"
                                } else {
                                    "unified"
                                },
                                if settings.diff_whitespace == WhitespaceMode::None {
                                    ""
                                } else {
                                    " · whitespace ignored"
                                }
                            )),
                    )
                    .child(preview),
            ),
    );

    let tool = {
        let entity = cx.entity();
        form::select(
            "settings-merge-tool".into(),
            TOOLS.iter().map(|tool| tool.label()),
            TOOLS
                .iter()
                .position(|candidate| *candidate == settings.merge_tool)
                .unwrap_or(0),
            false,
            move |index, _, cx| {
                let tool = TOOLS[index.min(TOOLS.len() - 1)];
                entity.update(cx, |view, cx| {
                    view.change(cx, |settings| {
                        settings.merge_tool = tool;
                        if tool != MergeTool::Custom {
                            settings.merge_command.clear();
                        }
                    });
                });
            },
        )
    };

    let merge_rows = match settings.merge_tool {
        MergeTool::Builtin => vec![form::row(
            "settings-merge-tool-row",
            "Resolve conflicts with",
            "Three-way editor inside GitWarp. No setup needed.",
            tool,
            false,
        )],
        MergeTool::Custom => {
            let command_text = settings.merge_command.clone();
            let missing: Vec<&str> = ["$LOCAL", "$REMOTE", "$MERGED"]
                .into_iter()
                .filter(|placeholder| !command_text.contains(placeholder))
                .collect();
            let bad = command_text.trim().is_empty() || !missing.is_empty();
            let mut rows = vec![form::row(
                "settings-merge-tool-row",
                "Resolve conflicts with",
                "Opened from the conflict banner; GitWarp waits until the tool exits.",
                tool,
                false,
            )];
            let reference = &view.merge_command;
            rows.push(
                div()
                    .w_full()
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .border_b_1()
                    .child(
                        div()
                            .text_size(FS_SM)
                            .child("Command"),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w_0().h(px(26.)).child(reference.clone()))
                            .child({
                                let entity = cx.entity();
                                Button::new("settings-merge-test")
                                    .size(ButtonSize::Small)
                                    .icon(IconName::Play)
                                    .label("Test")
                                    .disabled(bad)
                                    .on_click(move |_, _, cx: &mut App| {
                                        entity.update(cx, |view, cx| {
                                            view.start_merge_test(cx);
                                        });
                                    })
                            }),
                    )
                    .when_some(view.merge_test.clone(), |this, result| {
                        this.child(match result {
                            Ok(text) => form::success(text, cx),
                            Err(text) => form::error(text, cx),
                        })
                    })
                    .child(if command_text.trim().is_empty() {
                        form::error("Enter a command.", cx)
                    } else if !missing.is_empty() {
                        form::error(
                            format!(
                                "Missing {}. The tool would not receive all required files.",
                                missing.join(", ")
                            ),
                            cx,
                        )
                    } else {
                        form::success("All required placeholders present.", cx)
                    })
                    .child(form::help(
                        "Placeholders: $LOCAL your side · $REMOTE their side · $BASE common ancestor · $MERGED result file.",
                        cx,
                    ))
                    .into_any_element(),
            );
            rows
        }
        _ => {
            let command = settings.merge_tool.preset().to_owned();
            let mut rows = vec![form::row(
                "settings-merge-tool-row",
                "Resolve conflicts with",
                "Opened from the conflict banner; GitWarp waits until the tool exits.",
                tool,
                false,
            )];
            rows.push(
                div()
                    .w_full()
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .border_b_1()
                    .child(div().text_size(FS_SM).child("Command"))
                    .child(
                        div()
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(super::R_SM)
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.bg)
                            .font_family(FONT_MONO)
                            .text_size(FS_SM)
                            .child(command),
                    )
                    .child(form::help(
                        "Choose “Custom command…” to edit the command.",
                        cx,
                    ))
                    .into_any_element(),
            );
            rows
        }
    };
    page = page.child(group("Merge tool", merge_rows));
    page.into_any_element()
}

fn diff_preview(settings: &crate::settings::Settings, cx: &Context<SettingsView>) -> AnyElement {
    let colors = cx.theme().colors;
    let size = px(f32::from(settings.mono_font_size));
    let row = px(f32::from(settings.mono_font_size) + 8.);
    let mut body = div()
        .flex()
        .flex_col()
        .py(px(4.))
        .font_family(FONT_MONO)
        .text_size(size);
    body = body.child(
        div()
            .h(row)
            .flex()
            .items_center()
            .px(px(8.))
            .bg(colors.diff_hunk_bg)
            .text_size(FS_SM)
            .text_color(colors.muted)
            .child("@@ -38,9 +38,14 @@ impl GraphLayout"),
    );
    let lines: [(&str, Option<u32>, Option<u32>, bool); 5] = [
        (
            "pub fn lane_x(&self, col: usize) -> f32 {",
            Some(38),
            Some(38),
            false,
        ),
        (
            "    col as f32 * LANE_W + LANE_W / 2.0",
            Some(39),
            None,
            true,
        ),
        ("    match self.collapse_as_ref() {", None, Some(39), false),
        (
            "        Some(collapsed) => collapsed.x(col),",
            None,
            Some(40),
            false,
        ),
        (
            "        None => col as f32 * LANE_W,",
            None,
            Some(41),
            false,
        ),
    ];
    for (text, old, new, removed) in lines {
        let background = if removed {
            colors.diff_del_bg
        } else if old.is_none() {
            colors.diff_add_bg
        } else {
            gpui::transparent_black()
        };
        let mut line = div()
            .h(row)
            .flex()
            .items_center()
            .whitespace_nowrap()
            .when(settings.diff_wrap, |this| this.whitespace_normal())
            .bg(background);
        if settings.diff_line_numbers {
            line = line
                .child(gutter(old, colors, size))
                .child(gutter(new, colors, size));
        }
        line = line
            .child(div().w(px(14.)).text_color(colors.muted).child(if removed {
                "-"
            } else if old.is_none() {
                "+"
            } else {
                " "
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(settings.diff_wrap, |this| this.whitespace_normal())
                    .child(text),
            );
        body = body.child(line);
    }
    if settings.diff_layout == DiffLayout::Split {
        body = body.child(
            div()
                .px(px(8.))
                .pt(px(4.))
                .text_size(FS_SM)
                .text_color(colors.muted)
                .child("Side-by-side preview uses the same rows in two columns."),
        );
    }
    body.into_any_element()
}

fn gutter(line: Option<u32>, colors: ui::ThemeColors, size: gpui::Pixels) -> AnyElement {
    div()
        .w(px(f32::from(size) * 2.4))
        .pr(px(8.))
        .text_size(size)
        .text_color(colors.muted)
        .text_align(gpui::TextAlign::Right)
        .child(line.map(|line| line.to_string()).unwrap_or_default())
        .into_any_element()
}

impl SettingsView {
    /// Starts the configured merge tool on three sample files, detached, and
    /// reports whether the process could be spawned.
    pub(super) fn start_merge_test(&mut self, cx: &mut Context<Self>) {
        let command = self.merge_command.read(cx).text().trim().to_owned();
        if command.is_empty() {
            return;
        }
        self.merge_test = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let program = first_word(&command);
            let result = cx
                .background_executor()
                .spawn(async move { start_sample_merge(&command) })
                .await;
            this.update(cx, |view, cx| {
                view.merge_test = Some(match result {
                    Ok(()) => Ok(SharedString::from(format!(
                        "Started {program} with sample files"
                    ))),
                    Err(error) => Err(SharedString::from(format!("Could not start: {error}"))),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The program name the result line shows.
fn first_word(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .unwrap_or(command)
        .to_owned()
}

/// Writes base/local/remote files with one conflicting line into a fresh
/// temporary directory and spawns the tool on them. It never waits.
fn start_sample_merge(command: &str) -> std::io::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("gitwarp-merge-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    let base = dir.join("base.txt");
    let local = dir.join("local.txt");
    let remote = dir.join("remote.txt");
    let merged = dir.join("merged.txt");
    std::fs::write(&base, "one\nshared\nthree\n")?;
    std::fs::write(&local, "one\nlocal\nthree\n")?;
    std::fs::write(&remote, "one\nremote\nthree\n")?;
    std::fs::write(&merged, "")?;

    let program = first_word(command);
    let mut command = command.to_owned();
    for (placeholder, path) in [
        ("$BASE", &base),
        ("$LOCAL", &local),
        ("$REMOTE", &remote),
        ("$MERGED", &merged),
    ] {
        command = substitute(&command, placeholder, path);
    }

    let mut shell = if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.arg("/C");
        shell
    } else {
        let mut shell = Command::new("sh");
        shell.arg("-c");
        shell
    };
    let mut child = shell
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // A missing tool only shows up as the shell's exit status, so watch the
    // child briefly: 127 (sh) / 9009 (cmd) mean "command not found".
    let not_found = if cfg!(windows) { 9009 } else { 127 };
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
    while std::time::Instant::now() < deadline {
        match child.try_wait()? {
            Some(status) => {
                return match status.code() {
                    Some(0) => Ok(()),
                    Some(code) if code == not_found => Err(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("command not found: {program}"),
                    )),
                    Some(code) => Err(std::io::Error::other(format!("exited with code {code}"))),
                    None => Ok(()),
                };
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    // Still running: reap it in the background so it does not linger as a
    // zombie after it exits.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Replaces one placeholder with the quoted path, also consuming quotes the
/// command already put around it.
fn substitute(command: &str, placeholder: &str, path: &Path) -> String {
    let quoted = quote_path(path);
    // Quoted forms first so their quotes are consumed; the bare form then
    // handles whatever is left.
    command
        .replace(&format!("\"{placeholder}\""), &quoted)
        .replace(&format!("'{placeholder}'"), &quoted)
        .replace(placeholder, &quoted)
}

/// Quotes a path for the platform shell.
fn quote_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const P: &str = "'/tmp/m.txt'";
    #[cfg(windows)]
    const P: &str = "\"/tmp/m.txt\"";

    #[test]
    fn substitute_quoted_and_bare() {
        let path = Path::new("/tmp/m.txt");
        assert_eq!(
            substitute("tool \"$MERGED\"", "$MERGED", path),
            format!("tool {P}")
        );
        assert_eq!(
            substitute("tool '$MERGED'", "$MERGED", path),
            format!("tool {P}")
        );
        assert_eq!(
            substitute("tool $MERGED", "$MERGED", path),
            format!("tool {P}")
        );
    }

    #[test]
    fn substitute_mixed_forms() {
        let path = Path::new("/tmp/m.txt");
        assert_eq!(
            substitute("tool \"$MERGED\" '$MERGED' $MERGED", "$MERGED", path),
            format!("tool {P} {P} {P}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn missing_tool_is_an_error() {
        let result = start_sample_merge("definitely-not-a-real-tool-xyz $MERGED");
        assert!(result.is_err());
    }
}
