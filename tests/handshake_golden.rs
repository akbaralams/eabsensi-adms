//! Balasan handshake dibandingkan byte-per-byte dengan keluaran kode PHP lama.
//! Fixture dibangkitkan oleh `tests/fixtures/generate_handshake.py` langsung dari
//! `eabsensi-99/app/Http/Controllers/IDeviceController.php`.

use eabsensi_adms::protocol::handshake::{Format, HandshakeDevice, Stamps, render};

const SERIAL: &str = "SN-CONTOH-0001";

fn device(stamps: Stamps) -> HandshakeDevice {
    HandshakeDevice {
        serial: SERIAL.to_owned(),
        stamps,
        poll_delay_seconds: 30,
        error_delay_seconds: 60,
        offset_minutes: 420, // Asia/Jakarta -> "TimeZone=7"
    }
}

fn with_stamps() -> HandshakeDevice {
    device(Stamps {
        attlog: Some("111".to_owned()),
        operlog: Some("222".to_owned()),
        photo: Some("333".to_owned()),
        biodata: Some("444".to_owned()),
    })
}

fn without_stamps() -> HandshakeDevice {
    device(Stamps::default())
}

macro_rules! golden {
    ($name:ident, $format:expr, $device:expr, $file:literal) => {
        #[test]
        fn $name() {
            let expected: &[u8] = include_bytes!(concat!("fixtures/handshake/", $file));
            assert_eq!(render($format, &$device).as_bytes(), expected);
        }
    };
}

golden!(
    basic_dengan_stamp,
    Format::Basic,
    with_stamps(),
    "basic_stamps.txt"
);
golden!(
    basic_tanpa_stamp,
    Format::Basic,
    without_stamps(),
    "basic_none.txt"
);
golden!(
    att_dengan_stamp,
    Format::Att,
    with_stamps(),
    "att_stamps.txt"
);
golden!(
    att_tanpa_stamp,
    Format::Att,
    without_stamps(),
    "att_none.txt"
);
golden!(
    push23_dengan_stamp,
    Format::Push23,
    with_stamps(),
    "push23_stamps.txt"
);
golden!(
    push23_tanpa_stamp,
    Format::Push23,
    without_stamps(),
    "push23_none.txt"
);
