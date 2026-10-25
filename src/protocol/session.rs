//! Balasan endpoint pendamping: `push`, `rtdata`, `registry`, dan balasan unggahan.

use super::zk_time;

/// Balasan unggahan data: `OK:<n>\n` bila ada baris, selain itu `OK\n` (akhir `idata` lama).
pub fn render_upload_ok(count: usize) -> String {
    if count > 0 {
        format!("OK:{count}\n")
    } else {
        "OK\n".to_owned()
    }
}

/// Balasan `POST /iclock/push`. Baris terakhir tanpa LF, persis kode lama.
pub fn render_push(
    server_version: &str,
    error_delay_seconds: i16,
    poll_delay_seconds: i16,
    session_id: &str,
) -> String {
    format!(
        "ServerVersion={server_version}\n\
         ServerName=ADMS\n\
         PushVersion={server_version}\n\
         ErrorDelay={error_delay_seconds}\n\
         RequestDelay={poll_delay_seconds}\n\
         TransTimes=00:00\t14:00\n\
         TransInterval=1\n\
         TransTables=User\tTransaction\n\
         SessionID={session_id}\n\
         TimeoutSec=10"
    )
}

/// Balasan `GET /iclock/rtdata?type=time` (tanpa LF akhir).
pub fn render_rtdata_time(zk_now: i64, offset_minutes: i32) -> String {
    format!(
        "DateTime={zk_now},ServerTZ={}",
        zk_time::hhmm(offset_minutes)
    )
}

/// Memetakan `DeviceType` dari `registry` ke nilai `devices.device_type`: `1` adalah mesin
/// kontrol akses (`acc`, seperti kode lama), `att` dan `0` mesin absensi. Selain itu tidak
/// mengubah tipe yang tersimpan.
pub fn registry_device_type(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        "1" => Some("acc"),
        "att" | "0" => Some("att"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balasan_unggahan() {
        assert_eq!(render_upload_ok(0), "OK\n");
        assert_eq!(render_upload_ok(1), "OK:1\n");
        assert_eq!(render_upload_ok(250), "OK:250\n");
    }

    #[test]
    fn balasan_push_persis() {
        assert_eq!(
            render_push("2.3.0", 90, 30, "AbC123xYz9"),
            "ServerVersion=2.3.0\nServerName=ADMS\nPushVersion=2.3.0\nErrorDelay=90\nRequestDelay=30\n\
             TransTimes=00:00\t14:00\nTransInterval=1\nTransTables=User\tTransaction\n\
             SessionID=AbC123xYz9\nTimeoutSec=10"
        );
    }

    #[test]
    fn balasan_rtdata_time() {
        assert_eq!(
            render_rtdata_time(785_149_992, 420),
            "DateTime=785149992,ServerTZ=+0700"
        );
        assert_eq!(render_rtdata_time(1, -210), "DateTime=1,ServerTZ=-0330");
    }

    #[test]
    fn tipe_mesin_dari_registry() {
        assert_eq!(registry_device_type("1"), Some("acc"));
        assert_eq!(registry_device_type("att"), Some("att"));
        assert_eq!(registry_device_type("0"), Some("att"));
        assert_eq!(registry_device_type(" 1 "), Some("acc"));
        assert_eq!(registry_device_type("lain"), None);
        assert_eq!(registry_device_type(""), None);
    }
}
