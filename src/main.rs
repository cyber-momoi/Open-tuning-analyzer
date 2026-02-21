use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use std::{collections::{HashMap, HashSet}, io};

// --- 1. Physics Engine v5: Lightweight & Robust ---

// ノート名 -> インデックス (0-11)
fn get_note_mapping() -> HashMap<&'static str, u8> {
    let mut m = HashMap::new();
    m.insert("C", 0); m.insert("C#", 1); m.insert("Db", 1);
    m.insert("D", 2); m.insert("D#", 3); m.insert("Eb", 3);
    m.insert("E", 4); 
    m.insert("F", 5); m.insert("F#", 6); m.insert("Gb", 6);
    m.insert("G", 7); m.insert("G#", 8); m.insert("Ab", 8);
    m.insert("A", 9); m.insert("A#", 10); m.insert("Bb", 10);
    m.insert("B", 11);
    m
}

fn idx_to_note_name(idx: u8) -> &'static str {
    match idx % 12 {
        0 => "C", 1 => "Db", 2 => "D", 3 => "Eb", 4 => "E", 5 => "F",
        6 => "F#", 7 => "G", 8 => "Ab", 9 => "A", 10 => "Bb", 11 => "B",
        _ => "?",
    }
}

// コード定義辞書
// ここに定義を追加すれば、どんな変態コードも即座に対応可能
fn get_quality_intervals(quality: &str) -> Vec<u8> {
    match quality {
        // Basic
        "" | "M" | "maj"          => vec![0, 4, 7],
        "m" | "min" | "-"         => vec![0, 3, 7],
        "dim" | "o"               => vec![0, 3, 6],
        "aug" | "+"               => vec![0, 4, 8],
        "sus4" | "sus"            => vec![0, 5, 7],
        "sus2"                    => vec![0, 2, 7],
        
        // 7th
        "7" | "dom7"              => vec![0, 4, 7, 10],
        "M7" | "maj7" | "Maj7" | "jq" => vec![0, 4, 7, 11],
        "m7" | "min7" | "-7"      => vec![0, 3, 7, 10],
        "mM7" | "mMaj7"           => vec![0, 3, 7, 11],
        "dim7" | "o7"             => vec![0, 3, 6, 9],
        "m7-5" | "m7b5" | "half-dim" | "ø" => vec![0, 3, 6, 10],
        "7sus4"                   => vec![0, 5, 7, 10],
        "6"                       => vec![0, 4, 7, 9],
        "m6"                      => vec![0, 3, 7, 9],

        // Extended (9, 11, 13)
        "9"                       => vec![0, 4, 7, 10, 14],
        "add9"                    => vec![0, 4, 7, 14],
        "M9" | "maj9"             => vec![0, 4, 7, 11, 14],
        "m9" | "min9"             => vec![0, 3, 7, 10, 14], // ★Fm9対応
        "11"                      => vec![0, 4, 7, 10, 14, 17],
        "m11"                     => vec![0, 3, 7, 10, 14, 17],
        "13"                      => vec![0, 4, 7, 10, 14, 21],
        "M13"                     => vec![0, 4, 7, 11, 14, 21],

        // Altered / Fancy
        "7#9"                     => vec![0, 4, 7, 10, 15],
        "7b9"                     => vec![0, 4, 7, 10, 13],
        "7#5" | "aug7"            => vec![0, 4, 8, 10],
        
        // Default: Root + 5th (Power chord fallback)
        _ => vec![0, 7], 
    }
}

// オンコード対応パーサー
// "C/Bb" -> Root: C, Bass: Bb, Notes: [C, E, G, Bb]
fn parse_chord_v5(input: &str) -> (String, String, Vec<u8>) {
    let map = get_note_mapping();
    let s = input.trim();
    if s.is_empty() { return ("?".into(), "".into(), vec![]); }

    // 1. Slash Chord Split
    let parts: Vec<&str> = s.split('/').collect();
    let symbol = parts[0];
    let bass_str = if parts.len() > 1 { parts[1] } else { "" };

    // 2. Root Separation
    // 2文字目(#/b)チェック
    let (root_str, quality_str) = if symbol.len() > 1 {
        let second = symbol.chars().nth(1).unwrap();
        if second == '#' || second == 'b' {
            (&symbol[0..2], &symbol[2..])
        } else {
            (&symbol[0..1], &symbol[1..])
        }
    } else {
        (symbol, "")
    };

    let root_idx = match map.get(root_str) {
        Some(&i) => i,
        None => return (format!("Err:{}", root_str), "".into(), vec![]),
    };

    // 3. Generate Notes
    let intervals = get_quality_intervals(quality_str);
    let mut notes: Vec<u8> = intervals.iter().map(|&i| (root_idx + i) % 12).collect();

    // 4. Add Bass Note (if exists)
    if !bass_str.is_empty() {
        if let Some(&bass_idx) = map.get(bass_str) {
            // ベース音が構成音になければ追加
            if !notes.contains(&bass_idx) {
                // ベース音は通常最低音だが、集合演算上は単に追加でOK
                notes.insert(0, bass_idx); 
            }
        }
    }

    // 表示用ルート名 (オンコードならベースも表記)
    let display_name = if !bass_str.is_empty() {
        format!("{}/{}", root_str, bass_str)
    } else {
        root_str.to_string()
    };

    (display_name, quality_str.to_string(), notes)
}

fn get_scale_mask(root_u8: u8) -> HashSet<u8> {
    let intervals = [0, 2, 4, 5, 7, 9, 11];
    intervals.iter().map(|i| (root_u8 + i) % 12).collect()
}

// --- 1. Physics Engine Logic ---

fn calculate_tonal_depth(chord_notes: &[u8]) -> (Vec<(i32, &'static str)>, usize, bool) {
    let search_order = [
        (0, 0, "C"), (1, 7, "G"), (-1, 5, "F"),
        (2, 2, "D"), (-2, 10, "Bb"),
        (3, 9, "A"), (-3, 3, "Eb"),
        (4, 4, "E"), (-4, 8, "Ab"),
        (5, 11, "B"), (-5, 1, "Db"),
        (6, 6, "F#"), (-6, 6, "Gb"),
    ];
    
    let chord_set: HashSet<u8> = chord_notes.iter().cloned().collect();
    let total = chord_set.len();
    
    let mut max_score = 0;
    let mut candidates: Vec<(i32, &'static str)> = Vec::new();

    for (depth, r_idx, r_name) in search_order {
        let scale = get_scale_mask(r_idx);
        let score = chord_set.intersection(&scale).count();
        
        if score > max_score {
            max_score = score;
            candidates.clear();
            candidates.push((depth, r_name));
        } else if score == max_score {
            candidates.push((depth, r_name));
        }
    }
    
    let is_perfect = max_score == total;

    // ★ 修正点: 完全一致(Perfect)なら、最小作用の原理で1つに絞る
    if is_perfect && !candidates.is_empty() {
        // 絶対値が最も小さい(=Cに近い)ものを探す
        // sort_by_key は安定ソートなので、同距離(+6と-6など)ならsearch_order順が優先される
        candidates.sort_by_key(|k| k.0.abs());
        candidates.truncate(1); // 先頭の1つだけ残す
    }
    
    (candidates, max_score, is_perfect)
}

fn get_interval_label(root_idx: u8, target_idx: u8) -> &'static str {
    let diff = (target_idx + 12 - root_idx) % 12;
    match diff {
        0 => "R", 1 => "b9", 2 => "9", 3 => "m3", 4 => "M3", 5 => "11",
        6 => "#11", 7 => "5", 8 => "b13", 9 => "13", 10 => "m7", 11 => "M7", _ => "?"
    }
}

// --- 2. App State ---

struct App {
    input: String,
    progression: Vec<String>,
    tuning: Vec<u8>,
}

impl App {
    fn new() -> Self {
        Self {
            input: String::new(),
            // テスト: Fm9, オンコード(C/Bb), テンション(G13)
            progression: vec!["Fm9".into(), "C/Bb".into(), "G13".into(), "Dbdim7".into()],
            tuning: vec![0, 7, 2, 7, 9, 2], // C G D G A D
        }
    }

    fn submit(&mut self) {
        if !self.input.is_empty() {
            self.progression = self.input.split_whitespace().map(|s| s.to_string()).collect();
            self.input.clear();
        }
    }
}

// --- 3. UI ---

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    loop {
        terminal.draw(|f| ui(f, &mut app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Esc => break,
                    KeyCode::Enter => app.submit(),
                    KeyCode::Char(c) => app.input.push(c),
                    KeyCode::Backspace => { app.input.pop(); },
                    _ => {}
                }
            }
        }
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
        .split(f.size());

    let input_p = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title(" Input Chords "))
        .style(Style::default().fg(Color::Cyan));
    f.render_widget(input_p, chunks[0]);

    let header_cells = ["Chord", "Depth", "Local Key", "Notes", "6(C)", "5(G)", "4(D)", "3(G)", "2(A)", "1(D)"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app.progression.iter().map(|chord_str| {
        let (root_disp, _quality, notes) = parse_chord_v5(chord_str);
        
        let parts: Vec<&str> = root_disp.split('/').collect();
        let root_idx = *get_note_mapping().get(parts[0]).unwrap_or(&0);

        // ★ 変更点: 複数の候補を受け取る
        let (candidates, _score, perfect) = calculate_tonal_depth(&notes);
        
        // 代表値（スケール表示用）は、リストの最初のものを使う（あるいは0に近いものを選ぶロジックも可）
        // ここでは便宜上、先頭を使う
        let primary_candidate = candidates.first().unwrap_or(&(0, "C"));
        let key_root_idx = *get_note_mapping().get(primary_candidate.1).unwrap_or(&0);
        let scale_notes = get_scale_mask(key_root_idx);

        let mut cells = Vec::new();
        cells.push(Cell::from(chord_str.as_str()).style(Style::default().add_modifier(Modifier::BOLD)));
        
        // ★ Depth表示: 複数ある場合はカンマ区切りで表示
        // 例: "-3, +0, +3"
        let depth_str = candidates.iter()
            .map(|(d, _)| format!("{:+}", d))
            .collect::<Vec<_>>()
            .join(" "); // スペースかカンマで区切る
            
        let d_style = if perfect {
            // 完全一致が複数ある場合（C major chordなど）は、一番0に近いものを色判定基準にする
            let rep_depth = candidates[0].0; 
            let c = if rep_depth == 0 { Color::Green } else if rep_depth.abs() <= 1 { Color::Yellow } else { Color::Red };
            Style::default().fg(c)
        } else {
            // 不完全一致（dim7など）はマゼンタで「重ね合わせ」を強調
            Style::default().fg(Color::Magenta).add_modifier(Modifier::ITALIC)
        };
        cells.push(Cell::from(depth_str).style(d_style));

        // ★ Key表示: 複数ある場合はカンマ区切り
        let key_str = candidates.iter()
            .map(|(_, name)| *name)
            .collect::<Vec<_>>()
            .join(" ");
        cells.push(Cell::from(key_str));

        // ... (Notes, Strings表示は変更なし) ...
        
        let note_names: Vec<String> = notes.iter().map(|&i| idx_to_note_name(i).to_string()).collect();
        cells.push(Cell::from(note_names.join(" ")).style(Style::default().fg(Color::DarkGray)));

        for &t_idx in &app.tuning {
            let interval = get_interval_label(root_idx, t_idx);
            let in_chord = notes.contains(&t_idx);
            let in_scale = scale_notes.contains(&t_idx);

            let (txt, sty) = if in_chord {
                (interval.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else if in_scale {
                (interval.to_string(), Style::default().fg(Color::Cyan))
            } else {
                (format!("X({})", interval), Style::default().fg(Color::Red))
            };
            cells.push(Cell::from(txt).style(sty));
        }

        Row::new(cells)
    });

    let table = Table::new(rows, [
        Constraint::Percentage(12), Constraint::Percentage(10), Constraint::Percentage(10), Constraint::Percentage(14),
        Constraint::Percentage(9), Constraint::Percentage(9), Constraint::Percentage(9),
        Constraint::Percentage(9), Constraint::Percentage(9), Constraint::Percentage(9),
    ])
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Physics Engine v5 (Zero-Dependency) "));

    f.render_widget(table, chunks[1]);
    
    let footer = Paragraph::new("Ultra-Lightweight Mode | No ML, No Audio | Esc to Quit")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}
