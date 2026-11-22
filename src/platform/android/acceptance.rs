//! Local debug instrumentation only. This module is absent from release APKs.

use super::*;

const _: NativeMethod = native_method! {
    java_type = "com.kobaproduction.blazer.AcceptanceBridge",
    static extern fn execute(operation: JString, payload: JString) -> JString,
};

fn execute<'local>(
    env: &mut Env<'local>,
    _: JClass<'local>,
    operation: JString<'local>,
    payload: JString<'local>,
) -> jni::errors::Result<JString<'local>> {
    let operation = operation.mutf8_chars(env)?.to_string();
    let payload = payload.mutf8_chars(env)?.to_string();
    let result = match operation.as_str() {
        "import" => submit_import(payload),
        "status" => status(),
        "disconnect" => {
            with_session(|session| session.client.send(ClientCommand::Disconnect));
            status()
        }
        "shutdown" => shutdown(),
        _ => json!({"error": "unsupported acceptance operation"}),
    };
    env.new_string(result.to_string())
}

fn submit_import(payload: String) -> serde_json::Value {
    let request = with_session(|session| {
        session.client.send(ClientCommand::ImportText {
            title: "Android acceptance".into(),
            text: payload.into(),
        })
    });
    match request {
        Some(Ok(request)) => json!({"request": request.0}),
        _ => json!({"error": "acceptance import submission failed"}),
    }
}

fn status() -> serde_json::Value {
    with_session(|session| {
        let snapshot = session.client.snapshot();
        json!({"completed": snapshot.completed_request.map(|request| request.0),
            "error": snapshot.last_error.map(|error| format!("{error:?}")),
            "connection": format!("{:?}", snapshot.connection),
            "active": snapshot.active_mode.map(|mode| format!("{mode:?}")),
            "profiles": snapshot.profiles.len(), "cleanup": snapshot.cleanup_required})
    })
    .unwrap_or_else(|| json!({"error": "acceptance session missing"}))
}

fn shutdown() -> serde_json::Value {
    let session = SESSION.lock().ok().and_then(|mut session| session.take());
    let Some(AndroidSession {
        client,
        controller,
        refresh,
        periodic,
    }) = session
    else {
        return json!({"stopped": true});
    };
    drop((controller, refresh, periodic));
    match Arc::try_unwrap(client) {
        Ok(mut client) => match client.shutdown() {
            Ok(()) => json!({"stopped": true}),
            Err(error) => json!({"error": format!("{error:?}")}),
        },
        Err(_) => json!({"error": "acceptance client still attached"}),
    }
}
