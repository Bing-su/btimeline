use std::fmt::Write as _;

use anyhow::Result;
use itertools::Itertools;

use super::{Ability, Destination, Entry, FieldPattern, Jump, JumpWhen, Sync, Timeline};

fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}
fn at(value: f64) -> String {
    format!("{value:.1}")
}
fn quote(value: &str) -> String {
    format!("\"{value}\"")
}
fn comments(out: &mut String, text: &str) -> Result<()> {
    for line in text.split('\n') {
        writeln!(out, "# {line}")?;
    }
    Ok(())
}
fn field_pattern(value: &FieldPattern) -> String {
    match value {
        FieldPattern::One(v) => serde_json::Value::from(v.as_str()).to_string(),
        FieldPattern::Many(v) => format!(
            "[{}]",
            v.iter()
                .map(|s| serde_json::Value::from(s.as_str()).to_string())
                .join(", ")
        ),
    }
}
fn render_sync(sync: &Sync) -> String {
    match sync {
        Sync::Network(sync) => format!(
            "{:?} {{ {} }}",
            sync.log,
            sync.fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", field_pattern(v)))
                .join(", ")
        ),
        Sync::Regex(sync) => format!("sync /{}/", sync.regex),
    }
}
fn window(sync: &Sync) -> String {
    sync.window().map_or(String::new(), |[before, after]| {
        format!(" window {},{}", number(before), number(after))
    })
}
fn render_jump(jump: &Jump) -> String {
    let to = match &jump.to {
        Destination::Label(name) => quote(name),
        Destination::Time(time) => number(*time),
    };
    format!(
        "{} {to}",
        if matches!(jump.when, JumpWhen::Always) {
            "forcejump"
        } else {
            "jump"
        }
    )
}
fn ability_line(out: &mut String, ability: &Ability) -> Result<()> {
    writeln!(
        out,
        "# {} {}{}",
        ability.id,
        ability.name,
        ability
            .note
            .as_ref()
            .map_or(String::new(), |note| format!(": {note}"))
    )?;
    Ok(())
}

impl Timeline {
    pub(super) fn render(&self) -> Result<String> {
        let mut out = String::new();
        for name in &self.hide_names {
            writeln!(out, "hideall {}", quote(name))?;
        }
        for entry in &self.entries {
            match entry {
                Entry::Event {
                    at: time,
                    name,
                    duration,
                    sync,
                    jump,
                    note,
                } => {
                    if sync.as_ref().is_some_and(|s| !s.enabled())
                        && let Some(note) = note
                    {
                        comments(&mut out, note)?;
                    }
                    write!(out, "{} {}", at(*time), quote(name))?;
                    if let Some(sync) = sync
                        && sync.enabled()
                    {
                        write!(out, " {}", render_sync(sync))?;
                    }
                    if let Some(duration) = duration {
                        write!(out, " duration {duration}")?;
                    }
                    if let Some(sync) = sync
                        && sync.enabled()
                    {
                        write!(out, "{}", window(sync))?;
                    }
                    if let Some(jump) = jump {
                        write!(out, " {}", render_jump(jump))?;
                    }
                    if let Some(sync) = sync
                        && !sync.enabled()
                    {
                        write!(out, " #{}{}", render_sync(sync), window(sync))?;
                    }
                    if sync.as_ref().is_none_or(Sync::enabled)
                        && let Some(note) = note
                    {
                        write!(out, " # {}", note.replace('\n', "\n# "))?;
                    }
                    out.push('\n');
                }
                Entry::Label { at: time, name } => {
                    writeln!(out, "{} label {}", at(*time), quote(name))?;
                }
                Entry::Note { text } => comments(&mut out, text)?,
                Entry::GeneratorOptions {
                    targetable,
                    ignored_combatants,
                    phase_starts,
                } => {
                    if let Some(names) = targetable {
                        writeln!(out, "# -it {}", names.iter().map(|n| quote(n)).join(" "))?;
                    }
                    if let Some(names) = ignored_combatants {
                        writeln!(out, "# -ic {}", names.iter().map(|n| quote(n)).join(" "))?;
                    }
                    if let Some(starts) = phase_starts {
                        writeln!(
                            out,
                            "# -p {}",
                            starts
                                .iter()
                                .map(|p| format!("{}:{}", p.ability_id, number(p.at)))
                                .join(" ")
                        )?;
                    }
                }
                Entry::SyncOrder { enabled } => writeln!(
                    out,
                    "#cactbot-timeline-lint-{}-sync-order",
                    if *enabled { "enable" } else { "disable" }
                )?,
                Entry::AbilityCatalog { abilities, phase } => {
                    if let Some(phase) = phase {
                        writeln!(out, "# {phase}")?;
                    }
                    let ignored: Vec<_> =
                        abilities.iter().filter(|ability| ability.ignored).collect();
                    if !ignored.is_empty() {
                        writeln!(
                            out,
                            "# -ii {}",
                            ignored.iter().map(|a| a.id.as_str()).join(" ")
                        )?;
                        writeln!(out, "# IGNORED ABILITIES")?;
                        for ability in ignored {
                            ability_line(&mut out, ability)?;
                        }
                    }
                    writeln!(out, "# ALL ENCOUNTER ABILITIES")?;
                    for ability in abilities {
                        ability_line(&mut out, ability)?;
                    }
                }
            }
        }
        Ok(out)
    }
}
