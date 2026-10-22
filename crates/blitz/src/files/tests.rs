use std::path::PathBuf;

use vt::{Key, KeyInput, Mods, Palette};

use super::*;

fn key(key: Key, text: &'static str, ctrl: bool) -> KeyInput<'static> {
    KeyInput {
        vk: 0,
        scan: 0,
        extended: false,
        down: true,
        repeat: 1,
        mods: Mods {
            lctrl: ctrl,
            ..Mods::default()
        },
        locks: vt::Locks::default(),
        text,
        uc: 0,
        cs: 0,
        key,
        us_base: None,
    }
}

fn press(f: &mut FileView, k: Key) -> Act {
    f.key(&key(k, "", false))
}

fn pal() -> Palette {
    crate::theme::dark()
}

/// A folder of `dirs` and `files`, each file holding its own name.
fn tree(name: &str, dirs: &[&str], files: &[&str]) -> PathBuf {
    let d = std::env::temp_dir().join(format!("blitz-view-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    for x in dirs {
        std::fs::create_dir_all(d.join(x)).unwrap();
    }
    for x in files {
        std::fs::write(d.join(x), format!("{x}\nline 2\n")).unwrap();
    }
    d
}

/// Reads what the view asks for, as the app does on other threads.
fn settle(f: &mut FileView) {
    for _ in 0..4 {
        let n = f.needs();
        if n == Needs::default() {
            return;
        }
        if let Some(d) = n.list {
            let l = listing::read(&d);
            f.accept(Loaded::Listing(d, l));
        }
        if let Some(p) = n.preview {
            let r = load::read(&p);
            f.accept(Loaded::Preview(p, r));
        }
    }
}

/// The text of row `row` of a drawn view.
fn row(s: &vt::Snapshot, row: u16) -> String {
    let cols = usize::from(s.cols);
    let cells = &s.cells[usize::from(row) * cols..][..cols];
    let text: String = (cells.iter())
        .filter(|c| c.width > 0)
        .map(|c| match c.len {
            0 => " ".to_owned(),
            n => String::from_utf8_lossy(&c.text[..usize::from(n)]).into_owned(),
        })
        .collect();
    text.trim_end().to_owned()
}

fn drawn(f: &mut FileView, cols: u16, rows: u16) -> vt::Snapshot {
    let mut s = vt::Snapshot::default();
    f.draw(&mut s, cols, rows, &pal());
    s
}

fn sel(f: &FileView) -> Option<String> {
    f.selected().map(|e| e.label.clone())
}

#[test]
fn it_lists_the_folder_and_previews_the_highlighted_file() {
    let d = tree("basic", &["src"], &["Cargo.toml", "README.md"]);
    let mut f = FileView::new(d.clone());
    let _ = drawn(&mut f, 80, 10);
    settle(&mut f);
    assert_eq!(sel(&f).as_deref(), Some("src"));
    let s = drawn(&mut f, 80, 10);
    assert!(row(&s, 0).starts_with(" Files  "), "{}", row(&s, 0));
    assert!(row(&s, 0).ends_with("3 items"), "{}", row(&s, 0));
    let sep = std::path::MAIN_SEPARATOR;
    assert!(
        row(&s, 1).starts_with(&format!(" src{sep}")),
        "{}",
        row(&s, 1)
    );
    assert!(row(&s, 1).contains("Folder \u{b7} Enter opens it"));
    assert!(row(&s, 2).starts_with(" Cargo.toml"));
    assert!(row(&s, 2).contains("18 B"), "{}", row(&s, 2));
    assert!(row(&s, 9).starts_with(" Enter open"));

    press(&mut f, Key::Down);
    settle(&mut f);
    assert_eq!(sel(&f).as_deref(), Some("Cargo.toml"));
    let s = drawn(&mut f, 80, 10);
    assert!(row(&s, 1).ends_with("1  Cargo.toml"), "{}", row(&s, 1));
    assert!(row(&s, 2).ends_with("2  line 2"), "{}", row(&s, 2));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn enter_goes_into_folders_and_backspace_comes_back_to_where_it_was() {
    let d = tree("nav", &["a", "b/inner"], &["z.txt"]);
    let mut f = FileView::new(d.clone());
    settle(&mut f);
    press(&mut f, Key::Down);
    assert_eq!(sel(&f).as_deref(), Some("b"));
    assert_eq!(press(&mut f, Key::Enter), Act::Redraw);
    settle(&mut f);
    assert_eq!(f.dir, d.join("b"));
    assert_eq!(sel(&f).as_deref(), Some("inner"));
    assert_eq!(press(&mut f, Key::Backspace), Act::Redraw);
    settle(&mut f);
    assert_eq!(f.dir, d);
    assert_eq!(sel(&f).as_deref(), Some("b"), "the folder just left");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn typing_filters_and_esc_clears_before_it_closes() {
    let d = tree("filter", &[], &["alpha.rs", "beta.rs", "Alpine.md"]);
    let mut f = FileView::new(d.clone());
    settle(&mut f);
    f.key(&key(Key::Char('a'), "a", false));
    f.key(&key(Key::Char('l'), "L", false));
    assert_eq!(f.filter, "aL");
    let s = drawn(&mut f, 80, 6);
    assert!(row(&s, 0).ends_with("2 of 3"), "{}", row(&s, 0));
    assert_eq!(row(&s, 5), " Filter: aL");
    assert_eq!(s.cursor.map(|c| (c.0, c.1)), Some((11, 5)));
    assert_eq!(sel(&f).as_deref(), Some("alpha.rs"));
    press(&mut f, Key::Backspace);
    assert_eq!(f.filter, "a");
    assert_eq!(press(&mut f, Key::Escape), Act::Redraw);
    assert!(f.filter.is_empty());
    assert_eq!(press(&mut f, Key::Escape), Act::Close);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn hidden_entries_show_only_when_asked() {
    let d = tree("hidden", &[".git"], &[".env", "main.rs"]);
    let mut f = FileView::new(d.clone());
    settle(&mut f);
    assert_eq!(sel(&f).as_deref(), Some("main.rs"));
    f.key(&key(Key::Char('h'), "h", true));
    assert!(f.hidden && f.filter.is_empty());
    assert_eq!(sel(&f).as_deref(), Some("main.rs"), "the highlight stays");
    press(&mut f, Key::Home);
    assert_eq!(sel(&f).as_deref(), Some(".git"));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn the_preview_scrolls_and_opens_at_its_line() {
    let d = tree("scroll", &[], &[]);
    let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    std::fs::write(d.join("long.txt"), text).unwrap();
    let mut f = FileView::new(d.clone());
    let _ = drawn(&mut f, 80, 12);
    settle(&mut f);
    assert_eq!(press(&mut f, Key::Tab), Act::Redraw);
    assert_eq!(f.side, Side::Preview);
    press(&mut f, Key::PageDown);
    let s = drawn(&mut f, 80, 12);
    assert!(row(&s, 1).ends_with(" 10  line 10"), "{}", row(&s, 1));
    press(&mut f, Key::End);
    let s = drawn(&mut f, 80, 12);
    assert!(row(&s, 10).ends_with("100  line 100"), "{}", row(&s, 10));
    let ctrl_enter = key(Key::Enter, "", true);
    assert_eq!(f.key(&ctrl_enter), Act::Open(d.join("long.txt"), 91));
    assert_eq!(press(&mut f, Key::Escape), Act::Redraw);
    assert_eq!(f.side, Side::List);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn a_link_opens_the_file_at_its_line_marked() {
    let d = tree("at", &["src"], &["a.txt"]);
    let text: String = (1..=60).map(|i| format!("l{i}\n")).collect();
    std::fs::write(d.join("src/app.rs"), text).unwrap();
    let mut f = FileView::at(&d.join("src/app.rs"), Some(40));
    let _ = drawn(&mut f, 80, 14);
    settle(&mut f);
    assert_eq!(f.dir, d.join("src"));
    assert_eq!(sel(&f).as_deref(), Some("app.rs"));
    assert_eq!(f.side, Side::Preview);
    let s = drawn(&mut f, 80, 14);
    // A third of the way down, past the header.
    let marked = (1..13).find(|&r| row(&s, r).ends_with("40  l40")).unwrap();
    assert_eq!(marked, 5);
    let at = |r: u16| s.cells[usize::from(r) * 80 + 60].bg;
    assert_ne!(at(marked), at(marked + 1), "the line is marked");
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn clicks_pick_rows_and_double_clicks_open_them() {
    let d = tree("click", &["one", "two"], &["f.txt"]);
    let mut f = FileView::new(d.clone());
    let _ = drawn(&mut f, 80, 10);
    settle(&mut f);
    let _ = drawn(&mut f, 80, 10);
    assert_eq!(f.click(3, 3, false), Act::Redraw);
    assert_eq!(sel(&f).as_deref(), Some("f.txt"));
    assert_eq!(f.click(3, 0, false), Act::None, "the header");
    assert_eq!(f.click(3, 8, false), Act::None, "past the entries");
    assert_eq!(f.click(3, 2, true), Act::Redraw);
    settle(&mut f);
    assert_eq!(f.dir, d.join("two"));
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn what_was_read_for_somewhere_else_is_dropped() {
    let mut f = FileView::new(PathBuf::from("/nowhere/a"));
    let _ = f.needs();
    f.accept(Loaded::Listing(
        PathBuf::from("/nowhere/b"),
        Listing::default(),
    ));
    assert!(f.listing.is_none());
    f.accept(Loaded::Preview(
        PathBuf::from("/nowhere/a/x"),
        Preview::Binary(1),
    ));
    assert!(f.preview.is_none());
    assert_eq!(f.needs(), Needs::default(), "each folder is asked for once");
}

#[test]
fn binary_and_missing_files_say_so() {
    let d = tree("binary", &[], &[]);
    std::fs::write(d.join("a.bin"), b"\x7fELF\0\0\0").unwrap();
    let mut f = FileView::new(d.clone());
    settle(&mut f);
    let s = drawn(&mut f, 80, 6);
    assert!(
        row(&s, 1).contains("Binary file \u{b7} 7 B"),
        "{}",
        row(&s, 1)
    );
    let gone = FileView::new(d.join("gone"));
    let mut gone = gone;
    settle(&mut gone);
    let s = drawn(&mut gone, 80, 6);
    assert!(row(&s, 1).contains("It is not there any more"));
    assert_eq!(press(&mut gone, Key::Escape), Act::Close);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn tiny_panes_do_not_panic() {
    let d = tree("tiny", &["x"], &["y"]);
    let mut f = FileView::new(d.clone());
    settle(&mut f);
    for (c, r) in [(0, 0), (1, 1), (3, 2), (10, 3), (49, 4)] {
        let s = drawn(&mut f, c, r);
        assert_eq!(s.cells.len(), usize::from(c) * usize::from(r));
        for k in [Key::Down, Key::Tab, Key::PageDown, Key::Up] {
            press(&mut f, k);
        }
        let _ = f.click(0, 1, true);
        let _ = f.wheel(0, -3);
        settle(&mut f);
    }
    std::fs::remove_dir_all(&d).unwrap();
}
