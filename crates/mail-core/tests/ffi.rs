use mail_core::ffi::{
    mail_store_close, mail_store_free_string, mail_store_open, mail_store_request_json,
};
use std::ffi::{CStr, CString};

#[test]
fn c_abi_round_trip_returns_json_envelopes() {
    let path = CString::new(":memory:").unwrap();
    let store = unsafe { mail_store_open(path.as_ptr()) };
    assert!(!store.is_null());
    for (request, expected) in [
        (r#"{"op":"seed_demo"}"#, "true"),
        (
            r#"{"op":"list_threads","limit":10}"#,
            "Design review on Friday",
        ),
        (
            r#"{"op":"get_latest_message_for_thread","thread_id":"demo:1"}"#,
            "mockups",
        ),
        (r#"{"op":"missing_operation"}"#, "false"),
    ] {
        let request = CString::new(request).unwrap();
        let response = unsafe { mail_store_request_json(store, request.as_ptr()) };
        assert!(!response.is_null());
        let json = unsafe { CStr::from_ptr(response) }
            .to_str()
            .unwrap()
            .to_owned();
        assert!(json.contains(expected), "{json}");
        unsafe { mail_store_free_string(response) };
    }
    unsafe { mail_store_close(store) };
}
