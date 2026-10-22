//! MIDI input, parsing and the virtual keyboard feed.
//!
//! Live MIDI arrives on a `midir` callback thread and is pushed into a
//! bridge ring. The UI thread drains that bridge into the engine's `rtrb`
//! command ring, so the audio callback never touches a lock. The UI can also
//! push events directly, which makes the on-screen keyboard behave exactly
//! like hardware.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use midir::{MidiInput, MidiInputConnection};
use rtrb::{Consumer, Producer, RingBuffer};

pub const MIDI_RING_CAPACITY: usize = 4096;

/// A compact MIDI message understood by the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        value: u16,
    },
}

impl MidiMessage {
    /// Parse a raw MIDI byte stream (status byte first).
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        let channel = status & 0x0f;
        match status & 0xf0 {
            0x90 if bytes.len() >= 3 => {
                let velocity = bytes[2];
                if velocity == 0 {
                    Some(Self::NoteOff {
                        channel,
                        note: bytes[1],
                    })
                } else {
                    Some(Self::NoteOn {
                        channel,
                        note: bytes[1],
                        velocity,
                    })
                }
            }
            0x80 if bytes.len() >= 3 => Some(Self::NoteOff {
                channel,
                note: bytes[1],
            }),
            0xb0 if bytes.len() >= 3 => Some(Self::ControlChange {
                channel,
                controller: bytes[1],
                value: bytes[2],
            }),
            0xe0 if bytes.len() >= 3 => Some(Self::PitchBend {
                channel,
                value: ((bytes[2] as u16) << 7) | bytes[1] as u16,
            }),
            _ => None,
        }
    }

    pub fn note(&self) -> Option<u8> {
        match self {
            Self::NoteOn { note, .. } | Self::NoteOff { note, .. } => Some(*note),
            _ => None,
        }
    }

    /// Convert to the three-byte form used by plugin hosts.
    pub fn to_bytes(self) -> ([u8; 3], usize) {
        match self {
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => ([0x90 | channel, note, velocity], 3),
            Self::NoteOff { channel, note } => ([0x80 | channel, note, 0], 3),
            Self::ControlChange {
                channel,
                controller,
                value,
            } => ([0xb0 | channel, controller, value], 3),
            Self::PitchBend { channel, value } => (
                [0xe0 | channel, (value & 0x7f) as u8, (value >> 7) as u8],
                3,
            ),
        }
    }
}

/// UI/host side of the MIDI input subsystem.
pub struct MidiRuntime {
    /// Producer into the engine's MIDI ring (single producer: the UI thread).
    pub tx: Producer<MidiMessage>,
    rx: Option<Consumer<MidiMessage>>,
    bridge: Option<Consumer<MidiMessage>>,
    connection: Option<MidiInputConnection<()>>,
    pub ports: Vec<String>,
    pub connected: Option<usize>,
    pub error: Option<String>,
    pub activity: Arc<AtomicBool>,
    pub events_seen: u64,
}

impl MidiRuntime {
    pub fn new() -> Self {
        let (tx, rx) = RingBuffer::new(MIDI_RING_CAPACITY);
        let (ports, error) = match MidiInput::new("Replicant DAW") {
            Ok(input) => {
                let names = input
                    .ports()
                    .iter()
                    .map(|port| {
                        input
                            .port_name(port)
                            .unwrap_or_else(|_| "Unnamed MIDI port".to_string())
                    })
                    .collect();
                (names, None)
            }
            Err(error) => {
                log::warn!("MIDI input unavailable: {error}");
                (Vec::new(), Some(error.to_string()))
            }
        };

        Self {
            tx,
            rx: Some(rx),
            bridge: None,
            connection: None,
            ports,
            connected: None,
            error,
            activity: Arc::new(AtomicBool::new(false)),
            events_seen: 0,
        }
    }

    /// Hand the consumer to the engine (the engine owns it from then on).
    pub fn take_consumer(&mut self) -> Option<Consumer<MidiMessage>> {
        self.rx.take()
    }

    /// Refresh the list of available input ports.
    pub fn rescan(&mut self) {
        match MidiInput::new("Replicant DAW") {
            Ok(input) => {
                self.ports = input
                    .ports()
                    .iter()
                    .map(|port| {
                        input
                            .port_name(port)
                            .unwrap_or_else(|_| "Unnamed MIDI port".to_string())
                    })
                    .collect();
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn open(&mut self, index: usize) -> Result<(), String> {
        self.close();
        let mut input = MidiInput::new("Replicant DAW")
            .map_err(|error| format!("MIDI init failed: {error}"))?;
        input.ignore(midir::Ignore::None);
        let ports = input.ports();
        let Some(port) = ports.get(index) else {
            return Err(format!("MIDI port {index} is not available"));
        };

        let (mut bridge_tx, bridge_rx) = RingBuffer::new(MIDI_RING_CAPACITY);
        let activity = Arc::clone(&self.activity);
        let connection = input
            .connect(
                port,
                "replicant-midi-in",
                move |_stamp, bytes, _| {
                    if let Some(message) = MidiMessage::parse(bytes) {
                        let _ = bridge_tx.push(message);
                        activity.store(true, Ordering::Relaxed);
                    }
                },
                (),
            )
            .map_err(|error| format!("could not open MIDI port: {error}"))?;

        self.connection = Some(connection);
        self.bridge = Some(bridge_rx);
        self.connected = Some(index);
        self.error = None;
        Ok(())
    }

    pub fn close(&mut self) {
        if self.connection.take().is_some() {
            self.connected = None;
        }
        self.bridge = None;
    }

    /// Forward hardware events into the engine ring. Called once per UI frame.
    pub fn pump(&mut self) {
        let Some(bridge) = &mut self.bridge else {
            return;
        };
        for _ in 0..1024 {
            match bridge.pop() {
                Ok(message) => {
                    self.events_seen += 1;
                    let _ = self.tx.push(message);
                }
                Err(_) => break,
            }
        }
    }

    /// Inject an event as if it came from hardware (used by the UI keyboard).
    pub fn send(&mut self, message: MidiMessage) {
        self.events_seen += 1;
        let _ = self.tx.push(message);
    }

    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }
}

impl Default for MidiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_note_on_off_cc_and_bend() {
        assert_eq!(
            MidiMessage::parse(&[0x90, 60, 100]),
            Some(MidiMessage::NoteOn {
                channel: 0,
                note: 60,
                velocity: 100
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0x90, 60, 0]),
            Some(MidiMessage::NoteOff {
                channel: 0,
                note: 60
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0x83, 72, 0]),
            Some(MidiMessage::NoteOff {
                channel: 3,
                note: 72
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xb1, 7, 64]),
            Some(MidiMessage::ControlChange {
                channel: 1,
                controller: 7,
                value: 64
            })
        );
        assert_eq!(
            MidiMessage::parse(&[0xe0, 0, 96]),
            Some(MidiMessage::PitchBend {
                channel: 0,
                value: 0x3000
            })
        );
    }

    #[test]
    fn ignores_unsupported_messages() {
        assert_eq!(MidiMessage::parse(&[]), None);
        assert_eq!(MidiMessage::parse(&[0xf8]), None);
        assert_eq!(MidiMessage::parse(&[0x90, 60]), None);
    }

    #[test]
    fn round_trips_to_bytes() {
        let message = MidiMessage::NoteOn {
            channel: 2,
            note: 64,
            velocity: 90,
        };
        let (bytes, len) = message.to_bytes();
        assert_eq!(len, 3);
        assert_eq!(MidiMessage::parse(&bytes[..len]), Some(message));
    }
}
