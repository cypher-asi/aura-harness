//! Mesh mail delivery and the hub websocket.

use super::*;

/// Deliver one mesh message to the agent here it is for.
fn accept_mail(state: &RouterState, mesh: &MeshNode, mail: &MeshMail) -> MeshFrame {
    let ack = |delivered: bool, error: Option<String>| MeshFrame::MailAck {
        from_node: mail.from_node.clone(),
        id: mail.id,
        delivered,
        error,
    };
    if !mesh.seen.first_sight(mail) {
        return ack(true, None);
    }
    let delivered = remote_mail(mail).and_then(|remote| {
        if state.cloud.is_sealed(&remote.to) {
            return Err(format!("agent {} is sealed on this node", remote.to));
        }
        state.agent_host.deliver_remote(remote)
    });
    match delivered {
        Ok(()) => ack(true, None),
        Err(err) => {
            mesh.seen.forget(mail);
            ack(false, Some(err))
        }
    }
}

/// `POST /v1/mesh/mail`.
pub(in crate::gateway) async fn mesh_mail_handler(
    State(state): State<RouterState>,
    Json(mail): Json<MeshMail>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(accept_mail(&state, &state.mesh, &mail)))
}

/// `WS /v1/mesh`: the hub's socket.
pub(in crate::gateway) async fn mesh_ws_handler(
    State(state): State<RouterState>,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.cloud.enabled() {
        return not_cloud().into_response();
    }
    ws.on_upgrade(move |socket| run_mesh_socket(state, socket))
}

/// Host stats up the hub socket every [`HOST_STATS_EVERY`] until it closes.
fn spawn_host_stats(
    tx: mpsc::UnboundedSender<MeshFrame>,
    node: Arc<CloudNode>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if tx
                .send(MeshFrame::HostStats(node.host_stats().await))
                .is_err()
            {
                break;
            }
            tokio::time::sleep(HOST_STATS_EVERY).await;
        }
    })
}

/// Act on one frame from the hub.
fn on_frame(
    state: &RouterState,
    mesh: &MeshNode,
    tx: &mpsc::UnboundedSender<MeshFrame>,
    frame: MeshFrame,
) {
    match frame {
        MeshFrame::Directory { peers } => {
            let map = peers
                .into_iter()
                .filter_map(|peer| AgentId::from_hex(&peer.agent_id).ok().map(|id| (id, peer)))
                .collect();
            *mesh
                .directory
                .write()
                .unwrap_or_else(PoisonError::into_inner) = map;
        }
        MeshFrame::Mail(mail) => {
            let _ = tx.send(accept_mail(state, mesh, &mail));
        }
        MeshFrame::Ping { ms } => {
            let _ = tx.send(MeshFrame::Pong { ms });
        }
        _ => {}
    }
}

async fn run_mesh_socket(state: RouterState, socket: WebSocket) {
    let mesh = Arc::clone(&state.mesh);
    let (tx, mut rx) = mpsc::unbounded_channel::<MeshFrame>();
    let pending: Vec<MeshFrame> = std::mem::take(&mut *mesh.pending.lock());
    *mesh.uplink.lock() = Some(tx.clone());
    let (mut sink, mut stream) = socket.split();

    let _ = tx.send(MeshFrame::Hello {
        node_id: mesh.node_id(),
        protocol: MESH_PROTOCOL,
    });
    for frame in pending {
        let _ = tx.send(frame);
    }

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(WsMessage::Text(frame.to_text())).await.is_err() {
                break;
            }
        }
    });
    let stats = spawn_host_stats(tx.clone(), Arc::clone(&state.cloud));
    let heads = watch::spawn_head_watch(tx.clone(), state.clone());
    let processes = watch::spawn_process_watch(tx.clone(), state.clone());

    while let Some(Ok(message)) = stream.next().await {
        let WsMessage::Text(text) = message else {
            if matches!(message, WsMessage::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(frame) = MeshFrame::from_text(&text) else {
            tracing::debug!("skipping an unreadable mesh frame");
            continue;
        };
        on_frame(&state, &mesh, &tx, frame);
    }
    processes.abort();
    heads.abort();
    stats.abort();
    writer.abort();
    let mut uplink = mesh.uplink.lock();
    if uplink
        .as_ref()
        .is_some_and(|current| current.same_channel(&tx))
    {
        *uplink = None;
    }
}
