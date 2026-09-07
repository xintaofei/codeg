use sacp::{Client, ConnectTo};

use super::PidObservable;

/// An in-memory ACP agent transport for tests: instead of spawning a child
/// process, the client is wired to two duplex byte streams whose AGENT ends
/// the test drives itself. This is the seam that lets the REAL establishment
/// chain (`initialize` → `session/resume` → `session/load` → `session/new`,
/// exactly as `run_connection` implements it) be exercised against a fake
/// agent that records every method call and answers according to the scenario
/// under test.
///
/// The agent ends are exposed raw (bytes in / bytes out): a test speaks
/// newline-delimited JSON-RPC on them, mirroring what a real agent's stdio
/// carries. Nothing in this type touches the agent side — recording and
/// responses are entirely the test's job.
pub struct InMemoryAgentTransport {
    /// The agent's stdin: the transport writes CLIENT requests here (what a
    /// real agent process would read from its stdin).
    pub agent_stdin: tokio::io::DuplexStream,
    /// The agent's stdout: the transport reads AGENT responses from here
    /// (what a real agent process would write to its stdout).
    pub agent_stdout: tokio::io::DuplexStream,
}

impl ConnectTo<Client> for InMemoryAgentTransport {
    async fn connect_to(
        self,
        client: impl ConnectTo<sacp::role::acp::Agent>,
    ) -> Result<(), sacp::Error> {
        use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
        // Role `Agent` wire contract (mirrors the real stdio wiring in
        // sacp-tokio): `outgoing` = the agent's stdin (client requests arrive
        // here), `incoming` = the agent's stdout (agent responses are read
        // from here). Tokio duplex streams implement tokio's IO traits;
        // sacp's byte transport wants futures' — bridge with tokio-util's
        // compat adapters.
        let transport =
            sacp::ByteStreams::new(self.agent_stdin.compat_write(), self.agent_stdout.compat());
        ConnectTo::<Client>::connect_to(transport, client).await
    }
}

/// Pair the client-side transport halves with the agent ends handed to the
/// test's fake agent. The returned transport goes into
/// [`super::spawn_agent_connection_with_transport`]; the ends go to the fake.
pub fn in_memory_agent_pair(buffer: usize) -> (InMemoryAgentTransport, InMemoryAgentEnds) {
    let (agent_stdin, fake_stdin_reader) = tokio::io::duplex(buffer);
    let (fake_stdout_writer, agent_stdout) = tokio::io::duplex(buffer);
    (
        InMemoryAgentTransport {
            agent_stdin,
            agent_stdout,
        },
        InMemoryAgentEnds {
            client_to_agent: fake_stdin_reader,
            agent_to_client: fake_stdout_writer,
        },
    )
}

/// The agent-side ends of the in-memory wire (what the test's fake agent
/// reads and writes).
pub struct InMemoryAgentEnds {
    /// The fake agent reads client requests here.
    pub client_to_agent: tokio::io::DuplexStream,
    /// The fake agent writes responses here.
    pub agent_to_client: tokio::io::DuplexStream,
}

impl PidObservable for InMemoryAgentTransport {}
