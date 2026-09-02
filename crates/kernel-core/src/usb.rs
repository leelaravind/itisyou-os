//! USB descriptor parsing + HID boot-protocol decoding (pure logic, V0.6).
//!
//! The kernel's UHCI driver performs the transfers; this crate parses the
//! bytes they return (device/configuration/interface/endpoint descriptors) and
//! decodes HID boot keyboard/mouse reports, so the classification is
//! host-unit-testable and shared with any future controller (xHCI, ...).

/// Standard USB descriptor type codes (bDescriptorType).
pub mod desc_type {
    pub const DEVICE: u8 = 0x01;
    pub const CONFIGURATION: u8 = 0x02;
    pub const INTERFACE: u8 = 0x04;
    pub const ENDPOINT: u8 = 0x05;
    pub const HID: u8 = 0x21;
}

/// A parsed 18-byte USB device descriptor (the fields we use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDescriptor {
    pub usb_version: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet_size0: u8,
    pub vendor: u16,
    pub product: u16,
    pub num_configurations: u8,
}

impl DeviceDescriptor {
    /// Parse from the first 18 bytes of a GET_DESCRIPTOR(device) reply.
    pub fn parse(b: &[u8]) -> Option<DeviceDescriptor> {
        if b.len() < 18 || b[1] != desc_type::DEVICE {
            return None;
        }
        Some(DeviceDescriptor {
            usb_version: u16::from_le_bytes([b[2], b[3]]),
            class: b[4],
            subclass: b[5],
            protocol: b[6],
            max_packet_size0: b[7],
            vendor: u16::from_le_bytes([b[8], b[9]]),
            product: u16::from_le_bytes([b[10], b[11]]),
            num_configurations: b[17],
        })
    }
}

/// A parsed interface descriptor (the fields we use for class matching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDescriptor {
    pub interface_number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub num_endpoints: u8,
}

/// A parsed endpoint descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointDescriptor {
    /// bEndpointAddress: bit 7 set = IN.
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
    pub interval: u8,
}

impl EndpointDescriptor {
    pub fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }
    pub fn number(&self) -> u8 {
        self.address & 0x0F
    }
    /// Transfer type: 3 = interrupt.
    pub fn is_interrupt(&self) -> bool {
        self.attributes & 0x03 == 0x03
    }
}

/// HID class code (in an interface descriptor).
pub const CLASS_HID: u8 = 0x03;
/// HID boot-protocol subclass.
pub const HID_SUBCLASS_BOOT: u8 = 0x01;
/// HID boot protocols.
pub const HID_PROTOCOL_KEYBOARD: u8 = 0x01;
pub const HID_PROTOCOL_MOUSE: u8 = 0x02;

/// Walk a configuration descriptor's bytes, returning the first HID interface
/// and its first interrupt-IN endpoint (what a boot keyboard/mouse exposes).
/// Bounded by the buffer length, so a malformed descriptor cannot overrun.
pub fn find_hid_interrupt_in(config: &[u8]) -> Option<(InterfaceDescriptor, EndpointDescriptor)> {
    let mut i = 0usize;
    let mut current_iface: Option<InterfaceDescriptor> = None;
    while i + 2 <= config.len() {
        let len = config[i] as usize;
        let ty = config[i + 1];
        if len < 2 || i + len > config.len() {
            break; // malformed / truncated — stop safely.
        }
        match ty {
            desc_type::INTERFACE if len >= 9 => {
                current_iface = Some(InterfaceDescriptor {
                    interface_number: config[i + 2],
                    num_endpoints: config[i + 4],
                    class: config[i + 5],
                    subclass: config[i + 6],
                    protocol: config[i + 7],
                });
            }
            desc_type::ENDPOINT if len >= 7 => {
                let ep = EndpointDescriptor {
                    address: config[i + 2],
                    attributes: config[i + 3],
                    max_packet_size: u16::from_le_bytes([config[i + 4], config[i + 5]]),
                    interval: config[i + 6],
                };
                if let Some(iface) = current_iface {
                    if iface.class == CLASS_HID && ep.is_in() && ep.is_interrupt() {
                        return Some((iface, ep));
                    }
                }
            }
            _ => {}
        }
        i += len;
    }
    None
}

/// Decode an 8-byte HID boot keyboard report into the first pressed key's
/// ASCII (with the modifier's shift applied), if any. Report layout:
/// `[modifiers, reserved, key0..key5]`.
pub fn hid_keyboard_ascii(report: &[u8]) -> Option<u8> {
    if report.len() < 3 {
        return None;
    }
    let shift = report[0] & 0b0010_0010 != 0; // left/right shift
    for &usage in &report[2..report.len().min(8)] {
        if usage != 0 {
            return hid_usage_ascii(usage, shift);
        }
    }
    None
}

/// Map a HID keyboard usage ID to ASCII (US layout), shifted or not.
pub fn hid_usage_ascii(usage: u8, shift: bool) -> Option<u8> {
    let base = match usage {
        0x04..=0x1D => Some(b'a' + (usage - 0x04)), // a-z
        0x1E..=0x26 => Some(b'1' + (usage - 0x1E)), // 1-9
        0x27 => Some(b'0'),
        0x28 => Some(b'\n'), // enter
        0x2C => Some(b' '),  // space
        0x2D => Some(b'-'),
        0x2E => Some(b'='),
        _ => None,
    }?;
    if shift {
        Some(shift_ascii(base))
    } else {
        Some(base)
    }
}

fn shift_ascii(c: u8) -> u8 {
    match c {
        b'a'..=b'z' => c - 32, // uppercase
        b'1' => b'!',
        b'2' => b'@',
        b'3' => b'#',
        b'4' => b'$',
        b'5' => b'%',
        b'6' => b'^',
        b'7' => b'&',
        b'8' => b'*',
        b'9' => b'(',
        b'0' => b')',
        b'-' => b'_',
        b'=' => b'+',
        other => other,
    }
}

/// A decoded HID boot mouse report (3+ bytes): `[buttons, dx, dy, (wheel)]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidMouse {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub dx: i8,
    pub dy: i8,
}

pub fn hid_mouse(report: &[u8]) -> Option<HidMouse> {
    if report.len() < 3 {
        return None;
    }
    Some(HidMouse {
        left: report[0] & 1 != 0,
        right: report[0] & 2 != 0,
        middle: report[0] & 4 != 0,
        dx: report[1] as i8,
        dy: report[2] as i8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // QEMU usb-kbd device descriptor (18 bytes): VID 0x0627, PID 0x0001.
    const QEMU_KBD_DESC: [u8; 18] = [
        18, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 8, 0x27, 0x06, 0x01, 0x00, 0x00, 0x00, 0x01, 0x02,
        0x00, 0x01,
    ];

    #[test]
    fn parses_device_descriptor() {
        let d = DeviceDescriptor::parse(&QEMU_KBD_DESC).unwrap();
        assert_eq!(d.vendor, 0x0627);
        assert_eq!(d.product, 0x0001);
        assert_eq!(d.usb_version, 0x0200);
        assert_eq!(d.max_packet_size0, 8);
        assert_eq!(d.num_configurations, 1);
    }

    #[test]
    fn rejects_short_or_wrong_descriptor() {
        assert!(DeviceDescriptor::parse(&[0u8; 10]).is_none());
        let mut bad = QEMU_KBD_DESC;
        bad[1] = 0x02; // wrong type
        assert!(DeviceDescriptor::parse(&bad).is_none());
    }

    #[test]
    fn decodes_keyboard_report() {
        // 'a' pressed, no modifier.
        assert_eq!(hid_keyboard_ascii(&[0, 0, 0x04, 0, 0, 0, 0, 0]), Some(b'a'));
        // 'a' with left shift -> 'A'.
        assert_eq!(
            hid_keyboard_ascii(&[0x02, 0, 0x04, 0, 0, 0, 0, 0]),
            Some(b'A')
        );
        // no key.
        assert_eq!(hid_keyboard_ascii(&[0, 0, 0, 0, 0, 0, 0, 0]), None);
        // enter + space.
        assert_eq!(
            hid_keyboard_ascii(&[0, 0, 0x28, 0, 0, 0, 0, 0]),
            Some(b'\n')
        );
        assert_eq!(hid_keyboard_ascii(&[0, 0, 0x2C, 0, 0, 0, 0, 0]), Some(b' '));
    }

    #[test]
    fn decodes_mouse_report() {
        let m = hid_mouse(&[0x01, 5, 0xFB]).unwrap(); // left down, dx=5, dy=-5
        assert!(m.left && !m.right);
        assert_eq!(m.dx, 5);
        assert_eq!(m.dy, -5);
    }

    #[test]
    fn finds_hid_interrupt_endpoint() {
        // config(9) + interface(9, HID) + hid(9) + endpoint(7, IN interrupt).
        let cfg = [
            9, 0x02, 34, 0, 1, 1, 0, 0xA0, 50, // configuration
            9, 0x04, 0, 0, 1, 0x03, 0x01, 0x01, 0, // interface: HID boot kbd
            9, 0x21, 0x11, 0x01, 0, 1, 0x22, 63, 0, // HID descriptor
            7, 0x05, 0x81, 0x03, 8, 0, 10, // endpoint: EP1 IN, interrupt
        ];
        let (iface, ep) = find_hid_interrupt_in(&cfg).unwrap();
        assert_eq!(iface.class, CLASS_HID);
        assert_eq!(iface.protocol, HID_PROTOCOL_KEYBOARD);
        assert!(ep.is_in() && ep.is_interrupt());
        assert_eq!(ep.number(), 1);
        assert_eq!(ep.max_packet_size, 8);
    }

    #[test]
    fn hid_walk_tolerates_truncation() {
        // Declares length past the buffer — must not panic, returns None.
        let cfg = [9, 0x04, 0, 0, 1, 0x03, 0x01, 0x01, 0, 7, 0x05];
        assert!(find_hid_interrupt_in(&cfg).is_none());
    }
}
