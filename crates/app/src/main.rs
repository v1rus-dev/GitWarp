mod graph_paint;
mod history_view;
mod theme;

use gpui::{
    prelude::*, px, size, App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions,
};

use history_view::HistoryView;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("gitlane".into()),
                    ..Default::default()
                }),
                app_id: Some("dev.v1rus.gitlane".into()),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| HistoryView::open(&path, cx)),
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
