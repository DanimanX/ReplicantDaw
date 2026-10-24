//! End-to-end drag and drop between the session matrix and the arrangement.
//!
//! These tests drive the real drawing functions in the same order as the app,
//! with real pointer events, so a regression in panel ordering or in egui's
//! hover suppression cannot hide behind an isolated unit test.

use egui::{Pos2, Rect, Vec2};
use replicant_daw::audio::{AudioRuntime, MidiRuntime};
use replicant_daw::plugins::PluginRuntime;
use replicant_daw::samples::{ClipDrag, SampleClip};
use replicant_daw::state::UiState;
use replicant_daw::studio::Studio;
use replicant_daw::theme;
use replicant_daw::ui;

struct Harness {
    studio: Studio,
    state: UiState,
    audio: AudioRuntime,
    ctx: egui::Context,
    screen: Rect,
    matrix: Rect,
    lanes: Rect,
}

impl Harness {
    fn new() -> Self {
        let (plugins, _input, _output) = PluginRuntime::start(48_000.0);
        let mut studio = Studio::new(MidiRuntime::new(), plugins);
        let mut state = UiState::new();
        let mut audio = AudioRuntime::start_silent(Some("test".into()));
        theme::install(&egui::Context::default());

        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1500.0, 900.0));
        let matrix = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 860.0));
        let lanes = Rect::from_min_max(Pos2::new(720.0, 0.0), screen.max);

        // Give the session a sample so there is something to drag.
        let clip = SampleClip::from_tone("DRAGGED", 220.0, 0.5, 48_000);
        studio.assign_sample(&mut state, &mut audio, 1, 1, clip);

        Self {
            studio,
            state,
            audio,
            ctx: egui::Context::default(),
            screen,
            matrix,
            lanes,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(self.screen),
            events,
            ..Default::default()
        };
        let Harness {
            studio,
            state,
            audio,
            ctx,
            matrix,
            lanes,
            ..
        } = self;
        if studio.clip_drag.is_some() {
            studio.clear_clip_drop();
        }
        let mut output = ctx.run_ui(input, |ui| {
            // Same order as the application: session first, arrangement after.
            ui::session::draw_matrix(ui, *matrix, state, studio, audio);
            ui::lanes::draw_lanes(ui, *lanes, state, studio, audio);
            replicant_daw::ui::clip_drag_overlay(ui, studio);
        });
        output.textures_delta.clear();
        if ctx.input(|input| input.pointer.any_released()) {
            studio.commit_clip_drag(state, audio);
        }
    }

    fn cell(&self, track: usize, scene: usize) -> Rect {
        let content = theme::panel_content(self.matrix);
        ui::session::cell_rect(content, track, scene)
    }

    fn lane(&self, track: usize) -> Rect {
        let content = theme::panel_content(self.lanes);
        ui::lanes::lane_rect(content, track)
    }
}

fn press(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    }
}

/// Press on `from`, move to `to` in steps, release.
fn drag(h: &mut Harness, from: Pos2, to: Pos2) {
    h.frame(Vec::new());
    h.frame(vec![egui::Event::PointerMoved(from), press(from, true)]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let pos = from + (to - from) * t;
        h.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    h.frame(vec![egui::Event::PointerMoved(to), press(to, false)]);
}

#[test]
fn a_sample_copies_from_the_matrix_to_a_lane() {
    let mut h = Harness::new();
    let from = h.cell(1, 1).center();
    let to = h.lane(3).center();

    drag(&mut h, from, to);

    assert!(
        h.state.sample_clip(1, 1).is_some(),
        "the matrix slot must keep its clip after copying it to the timeline"
    );
    let placed = h
        .state
        .arrangement
        .iter()
        .find(|clip| clip.track == 3)
        .expect("no arrangement clip was created");
    assert!(placed.start_step > 0, "the clip landed at step 0");
    assert!(h.studio.clip_drag.is_none(), "the drag was not cleared");
}

#[test]
fn an_arrangement_clip_copies_to_a_session_cell() {
    let mut h = Harness::new();
    // Long enough that its block is a comfortable press target.
    let clip = SampleClip::from_tone("TIMELINE", 330.0, 2.0, 48_000);
    h.state.add_arrangement_clip(2, 0, clip, true, 0.0);

    let from = Pos2::new(h.lane(2).left() + 8.0, h.lane(2).center().y);
    let to = h.cell(4, 4).center();
    drag(&mut h, from, to);

    assert_eq!(
        h.state.arrangement.len(),
        1,
        "the timeline block must stay where it is"
    );
    assert!(
        h.state.sample_clip(4, 4).is_some(),
        "no session sample was created"
    );
    assert!(h.studio.clip_drag.is_none(), "the drag was not cleared");
}

#[test]
fn dragging_into_empty_space_cancels_without_losing_the_clip() {
    let mut h = Harness::new();
    let from = h.cell(1, 1).center();
    // Release over the matrix panel background, outside every cell.
    let to = Pos2::new(h.matrix.right() - 4.0, h.matrix.bottom() - 4.0);
    drag(&mut h, from, to);

    assert!(
        h.state.sample_clip(1, 1).is_some(),
        "cancelling a drag must not move the clip"
    );
    assert!(h.state.arrangement.is_empty());
    assert!(h.studio.clip_drag.is_none());
}

#[test]
fn the_overlay_reports_the_target_while_dragging() {
    let mut h = Harness::new();
    let from = h.cell(1, 1).center();
    let to = h.lane(5).center();

    h.frame(Vec::new());
    h.frame(vec![egui::Event::PointerMoved(from), press(from, true)]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let pos = from + (to - from) * t;
        h.frame(vec![egui::Event::PointerMoved(pos)]);
    }
    // Mid-drag, before release: the source is still intact and the drop target
    // under the pointer is known.
    assert!(h.state.sample_clip(1, 1).is_some());
    assert!(
        matches!(
            h.studio.clip_drop,
            Some(replicant_daw::samples::DropTarget::Arrangement { track: 5, .. })
        ),
        "the arrangement drop target was not reported: {:?}",
        h.studio.clip_drop
    );

    h.frame(vec![egui::Event::PointerMoved(to), press(to, false)]);
    assert!(
        h.state.sample_clip(1, 1).is_some(),
        "the dragged clip must keep its slot"
    );
    assert_eq!(h.state.arrangement.len(), 1, "the drop was not committed");
}

#[test]
fn a_clip_from_the_sample_editor_can_be_dropped_on_a_lane() {
    let mut h = Harness::new();
    // The editor button sets the payload; simulate the drag once it starts.
    h.studio.clip_drag = Some(ClipDrag::Editor {
        clip: SampleClip::from_tone("EDITOR", 110.0, 0.5, 48_000),
    });
    let to = h.lane(6).center();
    h.frame(vec![egui::Event::PointerMoved(to), press(to, false)]);

    assert_eq!(h.state.arrangement.len(), 1);
    assert_eq!(h.state.arrangement[0].track, 6);
}

#[test]
fn a_fast_flick_still_copies_the_clip() {
    // Press on one frame, travel and release on the next: the gesture never
    // has a frame where the pointer is down *and* already moved, so waiting
    // for egui's drag threshold would drop it.
    let mut h = Harness::new();
    let from = h.cell(1, 1).center();
    let to = h.lane(4).center();

    h.frame(Vec::new());
    h.frame(vec![egui::Event::PointerMoved(from), press(from, true)]);
    h.frame(vec![egui::Event::PointerMoved(to), press(to, false)]);

    assert!(
        h.state.sample_clip(1, 1).is_some(),
        "the matrix slot must keep its sample"
    );
    assert_eq!(h.state.arrangement.len(), 1);
    assert_eq!(h.state.arrangement[0].track, 4);
}

#[test]
fn a_plain_click_still_launches_and_keeps_the_clip() {
    let mut h = Harness::new();
    let at = h.cell(1, 1).center();

    h.frame(Vec::new());
    h.frame(vec![egui::Event::PointerMoved(at), press(at, true)]);
    h.frame(vec![press(at, false)]);

    assert!(
        h.state.sample_clip(1, 1).is_some(),
        "a click must not move the clip"
    );
    assert!(h.state.arrangement.is_empty());
    assert!(h.studio.clip_drag.is_none(), "the click left a drag behind");
    // The cell stopped (it was launched by the seed) instead of re-launching.
    assert_eq!(h.state.launched[1], None);
}

#[test]
fn an_arrangement_clip_moves_along_its_lane() {
    let mut h = Harness::new();
    let clip = SampleClip::from_tone("SLIDE", 330.0, 2.0, 48_000);
    let id = h.state.add_arrangement_clip(2, 0, clip, true, 0.0);

    let lane = h.lane(2);
    let from = Pos2::new(lane.left() + 8.0, lane.center().y);
    let to = Pos2::new(lane.left() + lane.width() * 0.5, lane.center().y);
    drag(&mut h, from, to);

    let moved = h.state.arrangement_clip(id).expect("clip disappeared");
    assert!(
        moved.start_step > 64,
        "the clip did not move with the pointer (step {})",
        moved.start_step
    );
    assert_eq!(moved.track, 2);
}

#[test]
fn an_arrangement_clip_can_change_lane() {
    let mut h = Harness::new();
    let clip = SampleClip::from_tone("LANE", 330.0, 2.0, 48_000);
    let id = h.state.add_arrangement_clip(2, 0, clip, true, 0.0);

    let from = Pos2::new(h.lane(2).left() + 8.0, h.lane(2).center().y);
    let to = Pos2::new(
        h.lane(6).left() + h.lane(6).width() * 0.4,
        h.lane(6).center().y,
    );
    drag(&mut h, from, to);

    let moved = h.state.arrangement_clip(id).expect("clip disappeared");
    assert_eq!(moved.track, 6);
    assert!(moved.start_step > 32);
}

#[test]
fn a_midi_clip_copies_from_the_matrix_to_a_lane() {
    // The case the matrix has to support: a musical block, not just a sample.
    let mut h = Harness::new();
    let notes = vec![
        replicant_daw::audio::RecordedNote {
            note: 60,
            velocity: 100,
            start_step: 0,
            length_steps: 4,
        },
        replicant_daw::audio::RecordedNote {
            note: 67,
            velocity: 90,
            start_step: 8,
            length_steps: 2,
        },
    ];
    h.state.set_recorded(2, 2, notes.clone());
    h.state.set_clip_loop(2, 2, 0, 16);

    let from = h.cell(2, 2).center();
    let to = h.lane(5).center();
    drag(&mut h, from, to);

    assert_eq!(
        h.state.recorded_notes(2, 2),
        notes.as_slice(),
        "the matrix slot must keep its notes"
    );
    let placed = h
        .state
        .arrangement
        .iter()
        .find(|clip| clip.track == 5)
        .expect("no MIDI clip landed on the timeline");
    assert_eq!(placed.content.notes(), notes.as_slice());
    assert_eq!(placed.content.loop_region(), Some((0, 16)));
    assert_eq!(placed.length_steps, 16);
    assert!(h.studio.clip_drag.is_none());
}

#[test]
fn a_timeline_midi_block_moves_without_touching_the_matrix() {
    let mut h = Harness::new();
    let notes = vec![replicant_daw::audio::RecordedNote {
        note: 48,
        velocity: 80,
        start_step: 0,
        length_steps: 4,
    }];
    let id = h
        .state
        .add_arrangement_midi(3, 16, "SCENE".into(), notes.clone(), 0, 16);

    let lane = h.lane(3);
    let from = Pos2::new(lane.left() + lane.width() * (20.0 / 256.0), lane.center().y);
    let to = Pos2::new(lane.left() + lane.width() * 0.5, lane.center().y);
    drag(&mut h, from, to);

    let moved = h.state.arrangement_clip(id).expect("clip disappeared");
    assert!(
        moved.start_step > 64,
        "the block did not follow the pointer"
    );
    assert_eq!(moved.content.notes(), notes.as_slice());
    assert!(
        h.state.recorded_notes(3, 16).is_empty(),
        "moving a timeline block must not write into the matrix"
    );
}

#[test]
fn a_block_can_be_grabbed_anywhere_inside_itself() {
    // The lane used to stop reporting a target while the pointer was inside
    // the source block, so a wide clip could not be dragged at all.
    let mut h = Harness::new();
    let clip = SampleClip::from_tone("LONG", 330.0, 2.0, 48_000);
    let id = h.state.add_arrangement_clip(2, 64, clip, true, 0.0);

    let lane = h.lane(2);
    // Press half-way through the block, not on its left edge.
    let from = Pos2::new(lane.left() + lane.width() * (80.0 / 256.0), lane.center().y);
    let to = Pos2::new(
        lane.left() + lane.width() * (144.0 / 256.0),
        lane.center().y,
    );
    drag(&mut h, from, to);

    let moved = h.state.arrangement_clip(id).expect("clip disappeared");
    assert!(
        moved.start_step >= 128,
        "the block was stuck: a grab from its middle did not move it (step {})",
        moved.start_step
    );
}

#[test]
fn a_plain_click_on_a_timeline_block_does_not_move_it() {
    let mut h = Harness::new();
    let clip = SampleClip::from_tone("CLICK", 330.0, 2.0, 48_000);
    let id = h.state.add_arrangement_clip(2, 64, clip, true, 0.0);

    let lane = h.lane(2);
    // A point inside the block that is not on a beat boundary.
    let at = Pos2::new(lane.left() + lane.width() * (73.0 / 256.0), lane.center().y);
    h.frame(Vec::new());
    h.frame(vec![egui::Event::PointerMoved(at), press(at, true)]);
    h.frame(vec![press(at, false)]);

    let unchanged = h.state.arrangement_clip(id).expect("clip disappeared");
    assert_eq!(
        unchanged.start_step, 64,
        "a click grabbed the block and snapped it to another beat"
    );
    assert_eq!(h.state.arrangement.len(), 1);
}
