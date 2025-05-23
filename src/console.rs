use bevy::ecs::{
    component::Tick,
    system::{SystemMeta, SystemParam, ScheduleSystem},
    world::unsafe_world_cell::UnsafeWorldCell,
};
use bevy::ecs::resource::Resource;
use bevy::{input::keyboard::KeyboardInput, prelude::*, platform::collections::HashMap};
use bevy_egui::egui::{self, Align, ScrollArea, TextEdit};
use bevy_egui::egui::{text::LayoutJob, text_selection::CCursorRange};
use bevy_egui::egui::{Context, Id};
use bevy_egui::{
    egui::{epaint::text::cursor::CCursor, Color32, FontId, TextFormat},
    EguiContexts,
};
use clap::{builder::StyledStr, CommandFactory, FromArgMatches};
use shlex::Shlex;
use trie_rs::{Trie, TrieBuilder};
use std::collections::{BTreeMap, VecDeque};
use std::marker::PhantomData;
use std::mem;

use crate::ConsoleSet;

type ConsoleCommandEnteredReaderSystemParam = EventReader<'static, 'static, ConsoleCommandEntered>;

type PrintConsoleLineWriterSystemParam = EventWriter<'static, PrintConsoleLine>;

/// A super-trait for command like structures
pub trait Command: NamedCommand + CommandFactory + FromArgMatches + Sized + Resource {}
impl<T: NamedCommand + CommandFactory + FromArgMatches + Sized + Resource> Command for T {}

/// Trait used to allow uniquely identifying commands at compile time
pub trait NamedCommand {
    /// Return the unique command identifier (same as the command "executable")
    fn name() -> &'static str;
}

/// Executed parsed console command.
///
/// Used to capture console commands which implement [`CommandName`], [`CommandArgs`] & [`CommandHelp`].
/// These can be easily implemented with the [`ConsoleCommand`](bevy_console_derive::ConsoleCommand) derive macro.
///
/// # Example
///
/// ```
/// # use bevy_console::ConsoleCommand;
/// # use clap::Parser;
/// /// Prints given arguments to the console.
/// #[derive(Parser, ConsoleCommand)]
/// #[command(name = "log")]
/// struct LogCommand {
///     /// Message to print
///     msg: String,
///     /// Number of times to print message
///     num: Option<i64>,
/// }
///
/// fn log_command(mut log: ConsoleCommand<LogCommand>) {
///     if let Some(Ok(LogCommand { msg, num })) = log.take() {
///         log.ok();
///     }
/// }
/// ```
pub struct ConsoleCommand<'w, T> {
    command: Option<Result<T, clap::Error>>,
    console_line: EventWriter<'w, PrintConsoleLine>,
}

impl<'w, T> ConsoleCommand<'w, T> {
    /// Returns Some(T) if the command was executed and arguments were valid.
    ///
    /// This method should only be called once.
    /// Consecutive calls will return None regardless if the command occurred.
    pub fn take(&mut self) -> Option<Result<T, clap::Error>> {
        mem::take(&mut self.command)
    }

    /// Print `[ok]` in the console.
    pub fn ok(&mut self) {
        self.console_line.write(PrintConsoleLine::new("[ok]".into()));
    }

    /// Print `[failed]` in the console.
    pub fn failed(&mut self) {
        self.console_line
            .write(PrintConsoleLine::new("[failed]".into()));
    }

    /// Print a reply in the console.
    ///
    /// See [`reply!`](crate::reply) for usage with the [`format!`] syntax.
    pub fn reply(&mut self, msg: impl Into<StyledStr>) {
        self.console_line.write(PrintConsoleLine::new(msg.into()));
    }

    /// Print a reply in the console followed by `[ok]`.
    ///
    /// See [`reply_ok!`](crate::reply_ok) for usage with the [`format!`] syntax.
    pub fn reply_ok(&mut self, msg: impl Into<StyledStr>) {
        self.console_line.write(PrintConsoleLine::new(msg.into()));
        self.ok();
    }

    /// Print a reply in the console followed by `[failed]`.
    ///
    /// See [`reply_failed!`](crate::reply_failed) for usage with the [`format!`] syntax.
    pub fn reply_failed(&mut self, msg: impl Into<StyledStr>) {
        self.console_line.write(PrintConsoleLine::new(msg.into()));
        self.failed();
    }
}

pub struct ConsoleCommandState<T> {
    #[allow(clippy::type_complexity)]
    event_reader: <ConsoleCommandEnteredReaderSystemParam as SystemParam>::State,
    console_line: <PrintConsoleLineWriterSystemParam as SystemParam>::State,
    marker: PhantomData<T>,
}

unsafe impl<T: Command> SystemParam for ConsoleCommand<'_, T> {
    type State = ConsoleCommandState<T>;
    type Item<'w, 's> = ConsoleCommand<'w, T>;

    fn init_state(world: &mut World, system_meta: &mut SystemMeta) -> Self::State {
        let event_reader = ConsoleCommandEnteredReaderSystemParam::init_state(world, system_meta);
        let console_line = PrintConsoleLineWriterSystemParam::init_state(world, system_meta);
        ConsoleCommandState {
            event_reader,
            console_line,
            marker: PhantomData,
        }
    }

    #[inline]
    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Self::Item<'w, 's> {
        let mut event_reader = ConsoleCommandEnteredReaderSystemParam::get_param(
            &mut state.event_reader,
            system_meta,
            world,
            change_tick,
        );
        let mut console_line = PrintConsoleLineWriterSystemParam::get_param(
            &mut state.console_line,
            system_meta,
            world,
            change_tick,
        );

        let command = event_reader.read().find_map(|command| {
            if T::name() == command.command_name {
                let clap_command = T::command().no_binary_name(true);
                // .color(clap::ColorChoice::Always);
                let arg_matches = clap_command.try_get_matches_from(command.args.iter());

                debug!(
                    "Trying to parse as `{}`. Result: {arg_matches:?}",
                    command.command_name
                );

                match arg_matches {
                    Ok(matches) => {
                        return Some(T::from_arg_matches(&matches));
                    }
                    Err(err) => {
                        console_line.write(PrintConsoleLine::new(err.render()));
                        return Some(Err(err));
                    }
                }
            }
            None
        });

        ConsoleCommand {
            command,
            console_line,
        }
    }
}
/// Parsed raw console command into `command` and `args`.
#[derive(Clone, Debug, Event)]
pub struct ConsoleCommandEntered {
    /// the command definition
    pub command_name: String,
    /// Raw parsed arguments
    pub args: Vec<String>,
}

/// Events to print to the console.
#[derive(Clone, Debug, Eq, Event, PartialEq)]
pub struct PrintConsoleLine {
    /// Console line
    pub line: StyledStr,
}

impl PrintConsoleLine {
    /// Creates a new console line to print.
    pub const fn new(line: StyledStr) -> Self {
        Self { line }
    }
}

/// Console configuration
#[derive(Resource)]
pub struct ConsoleConfiguration {
    /// Registered keys for toggling the console
    pub keys: Vec<KeyCode>,
    /// Left position
    pub left_pos: f32,
    /// Top position
    pub top_pos: f32,
    /// Console height
    pub height: f32,
    /// Console width
    pub width: f32,
    /// Registered console commands
    pub commands: BTreeMap<&'static str, clap::Command>,
    /// Number of commands to store in history
    pub history_size: usize,
    /// Line prefix symbol
    pub symbol: String,
    /// Custom argument completions for commands.
    /// Key is the command, entries are potential completions.
    pub arg_completions: HashMap<String, Vec<String>>,
    /// Trie used for completions, autogenerated from registered console commands
    commands_trie: Trie<u8>,
}

impl Default for ConsoleConfiguration {
    fn default() -> Self {
        Self {
            keys: vec![KeyCode::Backquote],
            left_pos: 200.0,
            top_pos: 100.0,
            height: 400.0,
            width: 800.0,
            commands: BTreeMap::new(),
            history_size: 20,
            symbol: "$ ".to_owned(),
            arg_completions: HashMap::new(),
            commands_trie: TrieBuilder::new().build(),
        }
    }
}

impl Clone for ConsoleConfiguration {
    fn clone(&self) -> ConsoleConfiguration {
        ConsoleConfiguration {
            keys: self.keys.clone(),
            left_pos: self.left_pos.clone(),
            top_pos: self.top_pos.clone(),
            height: self.height.clone(),
            width: self.width.clone(),
            commands: self.commands.clone(),
            history_size: self.history_size.clone(),
            symbol: self.symbol.clone(),
            arg_completions: self.arg_completions.clone(),
            commands_trie: TrieBuilder::new().build(),
        }
    }
}

/// Add a console commands to Bevy app.
pub trait AddConsoleCommand {
    /// Add a console command with a given system.
    ///
    /// This registers the console command so it will print with the built-in `help` console command.
    ///
    /// # Example
    ///
    /// ```
    /// # use bevy::prelude::*;
    /// # use bevy_console::{AddConsoleCommand, ConsoleCommand};
    /// # use clap::Parser;
    /// App::new()
    ///     .add_console_command::<LogCommand, _>(log_command);
    /// #
    /// # /// Prints given arguments to the console.
    /// # #[derive(Parser, ConsoleCommand)]
    /// # #[command(name = "log")]
    /// # struct LogCommand;
    /// #
    /// # fn log_command(mut log: ConsoleCommand<LogCommand>) {}
    /// ```
    fn add_console_command<T: Command, Params>(
        &mut self,
        system: impl IntoScheduleConfigs<ScheduleSystem, Params>,
    ) -> &mut Self;
}

impl AddConsoleCommand for App {
    fn add_console_command<T: Command, Params>(
        &mut self,
        system: impl IntoScheduleConfigs<ScheduleSystem, Params>,
    ) -> &mut Self {
        let sys = move |mut config: ResMut<ConsoleConfiguration>| {
            let command = T::command().no_binary_name(true);
            // .color(clap::ColorChoice::Always);
            let name = T::name();
            if config.commands.contains_key(name) {
                warn!(
                    "console command '{}' already registered and was overwritten",
                    name
                );
            }
            config.commands.insert(name, command);
        };

        let build_command_trie = move |mut config: ResMut<ConsoleConfiguration>| {
            let mut trie_builder = TrieBuilder::new();
            for cmd in config.commands.keys() {
                trie_builder.push(cmd);
            }
            config.commands_trie = trie_builder.build();
        };

        self.add_systems(Startup, (sys, build_command_trie).chain())
            .add_systems(Update, system.in_set(ConsoleSet::Commands))
    }
}

/// Console open state
#[derive(Default, Resource)]
pub struct ConsoleOpen {
    /// Console open
    pub open: bool,
}

#[derive(Resource)]
pub(crate) struct ConsoleState {
    pub(crate) buf: String,
    pub(crate) scrollback: Vec<StyledStr>,
    pub(crate) history: VecDeque<StyledStr>,
    pub(crate) history_index: usize,
    pub(crate) completions: Vec<String>,
}

impl Default for ConsoleState {
    fn default() -> Self {
        ConsoleState {
            buf: String::default(),
            scrollback: Vec::new(),
            history: VecDeque::from([StyledStr::new()]),
            history_index: 0,
            completions: Vec::new(),
        }
    }
}

pub(crate) fn console_ui(
    mut egui_context: EguiContexts,
    config: Res<ConsoleConfiguration>,
    mut keyboard_input_events: EventReader<KeyboardInput>,
    mut state: ResMut<ConsoleState>,
    mut command_entered: EventWriter<ConsoleCommandEntered>,
    mut console_open: ResMut<ConsoleOpen>,
) {
    let keyboard_input_events = keyboard_input_events.read().collect::<Vec<_>>();
    let ctx = egui_context.ctx_mut();

    let pressed = keyboard_input_events
        .iter()
        .any(|code| console_key_pressed(code, &config.keys));

    // always close if console open
    // avoid opening console if typing in another text input
    if pressed && (console_open.open || !ctx.wants_keyboard_input()) {
        console_open.open = !console_open.open;
    }

    if console_open.open {
        egui::Window::new("Console")
            .collapsible(false)
            .default_pos([config.left_pos, config.top_pos])
            .default_size([config.width, config.height])
            .resizable(true)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    let scroll_height = ui.available_height() - 30.0;

                    // Scroll area
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .max_height(scroll_height)
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                for line in &state.scrollback {
                                    let mut text = LayoutJob::default();

                                    text.append(
                                        &line.to_string(), //TOOD: once clap supports custom styling use it here
                                        0f32,
                                        TextFormat::simple(FontId::monospace(14f32), Color32::GRAY),
                                    );

                                    ui.label(text);
                                }
                            });

                            // Scroll to bottom if console just opened
                            if console_open.is_changed() {
                                ui.scroll_to_cursor(Some(Align::BOTTOM));
                            }
                        });

                    // Separator
                    ui.separator();

                    // Clear line on ctrl+c
                    if ui.input(|i| i.modifiers.ctrl & i.key_pressed(egui::Key::C))
                    {
                        state.buf.clear();
                        return;
                    }
                    
                    // Clear history on ctrl+l
                    if ui.input(|i| i.modifiers.ctrl & i.key_pressed(egui::Key::L))
                    {
                        state.scrollback.clear();
                        return;
                    }

                    // Input
                    let text_edit = TextEdit::singleline(&mut state.buf)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true)
                        .font(egui::TextStyle::Monospace);

                    // Handle enter
                    let text_edit_response = ui.add(text_edit);
                    if text_edit_response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        if state.buf.trim().is_empty() {
                            state.scrollback.push(StyledStr::new());
                        } else {
                            let msg = format!("{}{}", config.symbol, state.buf);
                            state.scrollback.push(msg.into());
                            let cmd_string = state.buf.clone();
                            state.history.insert(1, cmd_string.into());
                            if state.history.len() > config.history_size + 1 {
                                state.history.pop_back();
                            }
                            state.history_index = 0;

                            let mut args = Shlex::new(&state.buf).collect::<Vec<_>>();

                            if !args.is_empty() {
                                let command_name = args.remove(0);
                                debug!("Command entered: `{command_name}`, with args: `{args:?}`");

                                let command = config.commands.get(command_name.as_str());

                                if command.is_some() {
                                    command_entered
                                        .write(ConsoleCommandEntered { command_name, args });
                                } else {
                                    debug!(
                                        "Command not recognized, recognized commands: `{:?}`",
                                        config.commands.keys().collect::<Vec<_>>()
                                    );

                                    state.scrollback.push("error: Invalid command".into());
                                }
                            }

                            state.buf.clear();
                        }
                    }

                    // Autocomplete line on tab
                    if ui.input(|i| i.key_pressed(egui::Key::Tab))
                    {
                        let line_words: Vec<&str> = state.buf.split_whitespace().collect();
                        let target_word = line_words.last().unwrap_or(&"").to_string();
                        let target_is_arg: bool = state.buf.contains(' ');

                        if state.completions.contains(&target_word) { // continue cycling through potential completions
                            let i = state.completions.iter().position(|x| x == &target_word).unwrap();
                            let full_word = match state.completions.get(i + 1) {
                                Some(x) => x.to_string(),
                                None => state.completions[0].to_string(),
                            };
                            let full_line = line_words.iter()
                                .enumerate()
                                .filter(|(i, _)| i != &(line_words.len() - 1))
                                .fold(String::new(), |acc, (_, x)| acc + &x + &" ")
                                + &full_word;
                            state.buf = full_line;
                        } else if target_is_arg { // create completion list for arguments
                            let Some(cmd) = line_words.get(0) else { return; };
                            let Some(arg_completions) = config.arg_completions.get(*cmd) else { return; };

                            if state.buf.ends_with(' ') {
                                if !arg_completions.is_empty() {
                                    let full_line = line_words.iter()
                                        .enumerate()
                                        .filter(|(i, _)| i != &(line_words.len()))
                                        .fold(String::new(), |acc, (_, x)| acc + &x + &" ")
                                        + &arg_completions[0];
                                    state.completions = arg_completions.clone();
                                    state.buf = full_line;
                                }
                            } else {
                                let mut trie_builder = TrieBuilder::new();
                                arg_completions.iter().for_each(|x| trie_builder.push(x));
                                let search = trie_builder.build().predictive_search(&target_word);
                                let completions: Vec<&str> = search.iter()
                                    .map(|x| std::str::from_utf8(x).unwrap())
                                    .collect();
                                if !completions.is_empty() {
                                    let full_line = line_words.iter()
                                        .enumerate()
                                        .filter(|(i, _)| i != &(line_words.len() - 1))
                                        .fold(String::new(), |acc, (_, x)| acc + &x + &" ")
                                        + &completions[0];
                                    state.completions = completions.iter().map(|x| x.to_string()).collect();
                                    state.buf = full_line;
                                }
                            };
                        } else { // create completion list for commands
                            if target_word == "" {
                                // separate logic, as trie_rs::Trie::predictive_search runtime panics on empty strings
                                state.completions = config.commands.keys().map(|x| x.to_string()).collect();
                                state.buf = state.completions[0].to_string();
                            } else {
                                let search = config.commands_trie.predictive_search(&target_word);
                                let completions: Vec<&str> = search.iter()
                                    .map(|x| std::str::from_utf8(x).unwrap())
                                    .collect();
                                if !completions.is_empty() {
                                    state.completions = completions.iter().map(|x| x.to_string()).collect();
                                    state.buf = completions[0].to_string();
                                }
                            }
                        } 
                    } else if ui.input(|i| !i.key_down(egui::Key::Tab) & !i.keys_down.is_empty()) {
                        // User pressed a key that isn't Tab.
                        // We reset the completion list, so that if they press tab later, we always regenerate a new completions list.
                        state.completions = Vec::new();
                    }

                    // Handle up and down through history
                    if text_edit_response.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::ArrowUp))
                        && state.history.len() > 1
                        && state.history_index < state.history.len() - 1
                    {
                        if state.history_index == 0 && !state.buf.trim().is_empty() {
                            *state.history.get_mut(0).unwrap() = state.buf.clone().into();
                        }

                        state.history_index += 1;
                        let previous_item = state.history.get(state.history_index).unwrap().clone();
                        state.buf = previous_item.to_string();

                        set_cursor_pos(ui.ctx(), text_edit_response.id, state.buf.len());
                    } else if text_edit_response.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::ArrowDown))
                        && state.history_index > 0
                    {
                        state.history_index -= 1;
                        let next_item = state.history.get(state.history_index).unwrap().clone();
                        state.buf = next_item.to_string();

                        set_cursor_pos(ui.ctx(), text_edit_response.id, state.buf.len());
                    }

                    // Focus on input
                    ui.memory_mut(|m| m.request_focus(text_edit_response.id));
                });
            });
    }
}

pub(crate) fn receive_console_line(
    mut console_state: ResMut<ConsoleState>,
    mut events: EventReader<PrintConsoleLine>,
) {
    for event in events.read() {
        let event: &PrintConsoleLine = event;
        console_state.scrollback.push(event.line.clone());
    }
}

fn console_key_pressed(keyboard_input: &KeyboardInput, configured_keys: &[KeyCode]) -> bool {
    if !keyboard_input.state.is_pressed() {
        return false;
    }

    for configured_key in configured_keys {
        if configured_key == &keyboard_input.key_code {
            return true;
        }
    }

    false
}

fn set_cursor_pos(ctx: &Context, id: Id, pos: usize) {
    if let Some(mut state) = TextEdit::load_state(ctx, id) {
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
        state.store(ctx, id);
    }
}

#[cfg(test)]
mod tests {
    use bevy::input::keyboard::{Key, NativeKey, NativeKeyCode};
    use bevy::input::ButtonState;

    use super::*;

    #[test]
    fn test_console_key_pressed_scan_code() {
        let input = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Xkb(41)),
            logical_key: Key::Unidentified(NativeKey::Xkb(41)),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        };

        let config = vec![KeyCode::Unidentified(NativeKeyCode::Xkb(41))];

        let result = console_key_pressed(&input, &config);
        assert!(result);
    }

    #[test]
    fn test_console_wrong_key_pressed_scan_code() {
        let input = KeyboardInput {
            key_code: KeyCode::Unidentified(NativeKeyCode::Xkb(42)),
            logical_key: Key::Unidentified(NativeKey::Xkb(42)),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        };

        let config = vec![KeyCode::Unidentified(NativeKeyCode::Xkb(41))];

        let result = console_key_pressed(&input, &config);
        assert!(!result);
    }

    #[test]
    fn test_console_key_pressed_key_code() {
        let input = KeyboardInput {
            key_code: KeyCode::Backquote,
            logical_key: Key::Character("`".into()),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        };

        let config = vec![KeyCode::Backquote];

        let result = console_key_pressed(&input, &config);
        assert!(result);
    }

    #[test]
    fn test_console_wrong_key_pressed_key_code() {
        let input = KeyboardInput {
            key_code: KeyCode::KeyA,
            logical_key: Key::Character("A".into()),
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        };

        let config = vec![KeyCode::Backquote];

        let result = console_key_pressed(&input, &config);
        assert!(!result);
    }

    #[test]
    fn test_console_key_right_key_but_not_pressed() {
        let input = KeyboardInput {
            key_code: KeyCode::Backquote,
            logical_key: Key::Character("`".into()),
            state: ButtonState::Released,
            window: Entity::PLACEHOLDER,
        };

        let config = vec![KeyCode::Backquote];

        let result = console_key_pressed(&input, &config);
        assert!(!result);
    }
}
