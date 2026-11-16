//! What the widgets for getting things done keep: a list of things to do and a
//! note, each a plain text file next to `ben.ini`, and a timer for stretches
//! of focus.
//!
//! The files are the user's: they are written by hand, in any editor. The app
//! only reads them, and changes one thing in one of them: the mark in front
//! of a task that was clicked.
//!
//!   todo.txt   one task per line; a line that starts with `x ` is done;
//!              a line that starts with `#` is a remark, not a task
//!   note.txt   shown as it is

use crate::util::data_dir;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub text: String,
    pub done: bool,
}

pub fn todo_file() -> PathBuf {
    data_dir().join("todo.txt")
}

pub fn note_file() -> PathBuf {
    data_dir().join("note.txt")
}

const FIRST_TODO: &str = "# Việc cần làm, mỗi dòng một việc. Dòng bắt đầu bằng \"x \" là việc đã xong.\nBấm vào một việc để đánh dấu đã xong\nBấm cây bút để sửa danh sách này\nx Bật widget việc cần làm\n";
const FIRST_NOTE: &str = "Ghi chú của bạn nằm trong file note.txt.\nBấm vào đây để mở file và sửa.\n";

/// The task a line holds, if it holds one.
fn task_of(line: &str) -> Option<Task> {
    let line = line.trim().trim_start_matches('\u{feff}');
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    match line.strip_prefix("x ").or_else(|| line.strip_prefix("X ")) {
        Some(rest) => Some(Task { text: rest.trim().to_string(), done: true }),
        None => Some(Task { text: line.to_string(), done: false }),
    }
}

/// The tasks in the text of a todo file, in the file's order.
pub fn parse_tasks(text: &str) -> Vec<Task> {
    text.lines().filter_map(task_of).collect()
}

/// The text of a todo file with its `index`th task marked done, or not done if it was.
/// Every other line, remarks and blank ones too, stays as it is.
pub fn toggled(text: &str, index: usize) -> String {
    let mut seen = 0;
    let mut out = String::with_capacity(text.len() + 2);
    for line in text.lines() {
        match task_of(line) {
            Some(task) => {
                if seen == index {
                    if !task.done {
                        out.push_str("x ");
                    }
                    out.push_str(&task.text);
                } else {
                    out.push_str(line);
                }
                seen += 1;
            }
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// A text file that is read again whenever it has been written.
pub struct Watched {
    path: PathBuf,
    /// What a file that does not exist yet starts as.
    first: &'static str,
    stamp: Option<SystemTime>,
    pub text: String,
}

impl Watched {
    pub fn todo() -> Self {
        Watched { path: todo_file(), first: FIRST_TODO, stamp: None, text: String::new() }
    }

    pub fn note() -> Self {
        Watched { path: note_file(), first: FIRST_NOTE, stamp: None, text: String::new() }
    }

    /// Reads the file if it has changed since the last look (making it first if it is not
    /// there). Returns true if the text is new.
    pub fn look(&mut self) -> bool {
        if !self.path.exists() {
            if let Err(e) = std::fs::write(&self.path, self.first) {
                crate::log!("could not make {}: {e}", self.path.display());
            }
        }
        let stamp = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if stamp.is_some() && stamp == self.stamp {
            return false;
        }
        self.stamp = stamp;
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        let new = text != self.text;
        self.text = text;
        new
    }

    /// Writes new text to the file.
    pub fn write(&mut self, text: String) {
        if let Err(e) = std::fs::write(&self.path, &text) {
            crate::log!("could not save {}: {e}", self.path.display());
            return;
        }
        self.stamp = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        self.text = text;
    }
}

/// What the focus timer is counting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Focus,
    Rest,
}

/// A timer for stretches of focus with a rest after each (the "Pomodoro" way of working).
/// It runs on the clock, not on frames: the app may sleep between two looks at it.
pub struct Focus {
    pub stage: Stage,
    /// When the stage ends, while the timer runs.
    until: Option<Instant>,
    /// What is left of the stage, while it does not.
    left: Duration,
    /// How long the stage is in all.
    whole: Duration,
}

fn minutes(count: f32) -> Duration {
    Duration::from_secs_f32(count.max(0.1) * 60.0)
}

impl Focus {
    pub fn new(focus_minutes: f32) -> Self {
        Focus { stage: Stage::Focus, until: None, left: minutes(focus_minutes), whole: minutes(focus_minutes) }
    }

    pub fn running(&self) -> bool {
        self.until.is_some()
    }

    /// Whole seconds left of the stage, rounded up, as the timer shows them.
    pub fn seconds_left(&self, now: Instant) -> u32 {
        let left = match self.until {
            Some(until) => until.saturating_duration_since(now),
            None => self.left,
        };
        left.as_secs_f32().ceil() as u32
    }

    /// How much of the stage has gone by, 0 to 1.
    pub fn share(&self, now: Instant) -> f32 {
        let left = match self.until {
            Some(until) => until.saturating_duration_since(now),
            None => self.left,
        };
        (1.0 - left.as_secs_f32() / self.whole.as_secs_f32().max(1.0)).clamp(0.0, 1.0)
    }

    /// Starts the timer, or stops it where it is.
    pub fn toggle(&mut self, now: Instant) {
        match self.until.take() {
            Some(until) => self.left = until.saturating_duration_since(now),
            None => self.until = Some(now + self.left),
        }
    }

    /// Back to the start of a stretch of focus, not running.
    pub fn reset(&mut self, focus_minutes: f32) {
        *self = Focus::new(focus_minutes);
    }

    /// The settings changed: a stage that has not been started takes its new length.
    pub fn resize(&mut self, focus_minutes: f32, rest_minutes: f32) {
        let whole = minutes(if self.stage == Stage::Focus { focus_minutes } else { rest_minutes });
        if self.until.is_none() && self.left == self.whole {
            self.left = whole;
            self.whole = whole;
        }
    }

    /// Looks at the clock. When the stage has just run out, the next one starts counting at
    /// once, and the stage that ended is returned.
    pub fn look(&mut self, now: Instant, focus_minutes: f32, rest_minutes: f32) -> Option<Stage> {
        let until = self.until?;
        if now < until {
            return None;
        }
        let ended = self.stage;
        self.stage = if ended == Stage::Focus { Stage::Rest } else { Stage::Focus };
        self.whole = minutes(if self.stage == Stage::Focus { focus_minutes } else { rest_minutes });
        self.left = self.whole;
        self.until = Some(now + self.whole);
        Some(ended)
    }
}

/// How much of today, of this week (from Monday), of this month and of this year has gone
/// by, each 0 to 1. `weekday`: 0 = Monday. `seconds`: since midnight.
pub fn gone_by(year: i32, month: u32, day: u32, weekday: u32, seconds: u32) -> [f32; 4] {
    let today = seconds as f32 / 86_400.0;
    let days_before: u32 = (1..month).map(|m| crate::dates::days_in_month(year, m)).sum();
    let in_year = if crate::dates::is_leap(year) { 366.0 } else { 365.0 };
    [
        today.clamp(0.0, 1.0),
        ((weekday as f32 + today) / 7.0).clamp(0.0, 1.0),
        (((day - 1) as f32 + today) / crate::dates::days_in_month(year, month) as f32).clamp(0.0, 1.0),
        (((days_before + day - 1) as f32 + today) / in_year).clamp(0.0, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_are_read_from_the_lines() {
        let text = "\u{feff}# things\n\nBuy milk\nx Call home\n   X  Pay rent  \n#x not a task\nxylophone lessons\n";
        let tasks = parse_tasks(text);
        assert_eq!(tasks.len(), 4);
        assert_eq!(tasks[0], Task { text: "Buy milk".into(), done: false });
        assert_eq!(tasks[1], Task { text: "Call home".into(), done: true });
        assert_eq!(tasks[2], Task { text: "Pay rent".into(), done: true });
        // "x" only marks a task done when a space follows it.
        assert_eq!(tasks[3], Task { text: "xylophone lessons".into(), done: false });
        assert!(parse_tasks("").is_empty());
    }

    #[test]
    fn a_click_changes_one_mark_and_nothing_else() {
        let text = "# things\n\nBuy milk\nx Call home\n";
        let done = toggled(text, 0);
        assert_eq!(done, "# things\n\nx Buy milk\nx Call home\n");
        assert_eq!(toggled(&done, 0), text);
        assert_eq!(toggled(text, 1), "# things\n\nBuy milk\nCall home\n");
        // No such task: the same text (a last line without its newline gets one).
        assert_eq!(toggled(text, 7), text);
        assert_eq!(toggled("Buy milk", 0), "x Buy milk\n");
        assert_eq!(parse_tasks(&toggled(text, 1)).iter().filter(|t| t.done).count(), 0);
    }

    #[test]
    fn the_first_files_read_as_meant() {
        let tasks = parse_tasks(FIRST_TODO);
        assert_eq!(tasks.len(), 3);
        assert!(tasks[2].done && !tasks[0].done);
    }

    #[test]
    fn focus_counts_down_rests_and_starts_over() {
        let start = Instant::now();
        let at = |seconds: f32| start + Duration::from_secs_f32(seconds);
        let mut focus = Focus::new(25.0);
        assert!(!focus.running());
        assert_eq!(focus.seconds_left(at(0.0)), 1500);
        assert_eq!(focus.share(at(0.0)), 0.0);
        // Not running: the clock does not matter.
        assert_eq!(focus.look(at(5000.0), 25.0, 5.0), None);
        focus.toggle(at(0.0));
        assert!(focus.running());
        assert_eq!(focus.seconds_left(at(60.2)), 1440);
        assert!((focus.share(at(750.0)) - 0.5).abs() < 0.001);
        // Stopped, it keeps what is left; started again, it goes on from there.
        focus.toggle(at(100.0));
        assert_eq!(focus.seconds_left(at(900.0)), 1400);
        focus.toggle(at(1000.0));
        assert_eq!(focus.look(at(2399.0), 25.0, 5.0), None);
        assert_eq!(focus.look(at(2400.5), 25.0, 5.0), Some(Stage::Focus));
        assert_eq!(focus.stage, Stage::Rest);
        assert!(focus.running());
        assert_eq!(focus.seconds_left(at(2400.5)), 300);
        assert_eq!(focus.look(at(2701.0), 25.0, 5.0), Some(Stage::Rest));
        assert_eq!(focus.stage, Stage::Focus);
        focus.reset(25.0);
        assert!(!focus.running() && focus.stage == Stage::Focus);
        assert_eq!(focus.seconds_left(at(9999.0)), 1500);
    }

    #[test]
    fn a_new_length_only_changes_a_stage_not_yet_started() {
        let now = Instant::now();
        let mut focus = Focus::new(25.0);
        focus.resize(50.0, 5.0);
        assert_eq!(focus.seconds_left(now), 3000);
        focus.toggle(now);
        focus.toggle(now + Duration::from_secs(600));
        focus.resize(25.0, 5.0);
        assert_eq!(focus.seconds_left(now), 2400, "a stage under way keeps its time");
    }

    #[test]
    fn how_much_has_gone_by() {
        // Noon on Thursday 1 January 2026.
        let [day, week, month, year] = gone_by(2026, 1, 1, 3, 43_200);
        assert!((day - 0.5).abs() < 1e-6);
        assert!((week - 3.5 / 7.0).abs() < 1e-6);
        assert!((month - 0.5 / 31.0).abs() < 1e-6);
        assert!((year - 0.5 / 365.0).abs() < 1e-6);
        // The last second of a leap year.
        let all = gone_by(2024, 12, 31, 1, 86_399);
        assert!(all[0] > 0.9999 && all[2] > 0.9999 && all[3] > 0.9999 && all[3] <= 1.0);
    }
}
