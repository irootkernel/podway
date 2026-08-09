//! Phase 4 singleton endpoint and peer UID boundary contracts.

#![forbid(unsafe_code)]

use nix::unistd::geteuid;
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    env, fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use podway_daemon::endpoint::{EndpointErrorV1, EndpointPathViolationV1, SingletonEndpointV1};
use podway_daemon::peer::{
    FixedPeerCredentialSourceV1, PeerCredentialErrorV1, PeerFrameAdmissionErrorV1,
    PeerUidVerificationErrorV1, PeerUidVerifierV1,
};
use podway_service::ServiceRuntimePathsV1;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct RuntimeFixture {
    root: PathBuf,
    runtime_root: PathBuf,
    paths: ServiceRuntimePathsV1,
}

impl RuntimeFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "podway-daemon-phase4-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture root must be created");

        let launch_agents = root.join("LaunchAgents");
        let application_support = root.join("ApplicationSupport");
        let logs = root.join("Logs");
        let runtime_root = short_runtime_directory(&root);
        for directory in [&launch_agents, &application_support, &logs] {
            fs::create_dir(directory).expect("fixture service directory must be created");
        }
        let paths = ServiceRuntimePathsV1::from_directories(
            launch_agents,
            application_support,
            logs,
            runtime_root.clone(),
        )
        .expect("fixture paths must be valid service paths");
        Self {
            root,
            runtime_root,
            paths,
        }
    }

    fn runtime_directory(&self) -> &Path {
        self.paths.runtime_directory().as_path()
    }

    fn socket_path(&self) -> &Path {
        self.paths.socket_path().as_path()
    }

    fn lock_path(&self) -> &Path {
        self.paths.global_lock_path().as_path()
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.runtime_root);
    }
}

fn short_runtime_directory(root: &Path) -> PathBuf {
    let mut digest = 0xcbf2_9ce4_8422_2325_u64;
    for byte in root.as_os_str().as_encoded_bytes() {
        digest ^= u64::from(*byte);
        digest = digest.wrapping_mul(0x0000_0100_0000_01b3);
    }
    env::temp_dir().join(format!("pw4r-{digest:016x}"))
}
fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("fixture path must exist")
        .permissions()
        .mode()
        & 0o777
}
#[derive(Debug, Eq, PartialEq)]
enum RustTokenV1 {
    Ident(String),
    StringLiteral(String),
    Punct(char),
    PathSeparator,
}

fn rust_tokens(source: &str) -> Vec<RustTokenV1> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        match bytes[cursor] {
            byte if byte.is_ascii_whitespace() => cursor += 1,
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor += 2;
                while cursor < bytes.len() && bytes[cursor] != b'\n' {
                    cursor += 1;
                }
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor += 2;
                let mut depth = 1;
                while depth > 0 {
                    match (bytes.get(cursor), bytes.get(cursor + 1)) {
                        (Some(b'/'), Some(b'*')) => {
                            depth += 1;
                            cursor += 2;
                        }
                        (Some(b'*'), Some(b'/')) => {
                            depth -= 1;
                            cursor += 2;
                        }
                        (Some(_), _) => cursor += 1,
                        (None, _) => panic!("unterminated block comment in daemon source"),
                    }
                }
            }
            b'"' | b'b' if bytes.get(cursor + 1) == Some(&b'"') => {
                let quote = if bytes[cursor] == b'b' {
                    cursor += 1;
                    cursor
                } else {
                    cursor
                };
                cursor += 1;
                let start = cursor;
                while cursor < bytes.len() && bytes[cursor] != b'"' {
                    if bytes[cursor] == b'\\' {
                        cursor += 1;
                    }
                    cursor += 1;
                }
                assert!(cursor < bytes.len(), "unterminated string in daemon source");
                tokens.push(RustTokenV1::StringLiteral(source[start..cursor].to_owned()));
                cursor = quote + (cursor - quote) + 1;
            }
            b'r' | b'b'
                if bytes[cursor] == b'r'
                    || (bytes[cursor] == b'b' && bytes.get(cursor + 1) == Some(&b'r')) =>
            {
                let raw_start = if bytes[cursor] == b'b' {
                    cursor + 1
                } else {
                    cursor
                };
                let mut quote = raw_start + 1;
                while bytes.get(quote) == Some(&b'#') {
                    quote += 1;
                }
                if bytes.get(quote) != Some(&b'"') {
                    let start = cursor;
                    cursor += 1;
                    while cursor < bytes.len()
                        && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
                    {
                        cursor += 1;
                    }
                    tokens.push(RustTokenV1::Ident(source[start..cursor].to_owned()));
                    continue;
                }
                let hashes = quote - raw_start - 1;
                let content_start = quote + 1;
                cursor = content_start;
                loop {
                    let Some(end_quote) = source[cursor..].find('"') else {
                        panic!("unterminated raw string in daemon source");
                    };
                    cursor += end_quote + 1;
                    if bytes[cursor..].starts_with(&vec![b'#'; hashes]) {
                        tokens.push(RustTokenV1::StringLiteral(
                            source[content_start..cursor - 1].to_owned(),
                        ));
                        cursor += hashes;
                        break;
                    }
                }
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = cursor;
                cursor += 1;
                while cursor < bytes.len()
                    && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
                {
                    cursor += 1;
                }
                tokens.push(RustTokenV1::Ident(source[start..cursor].to_owned()));
            }
            b':' if bytes.get(cursor + 1) == Some(&b':') => {
                tokens.push(RustTokenV1::PathSeparator);
                cursor += 2;
            }
            byte => {
                tokens.push(RustTokenV1::Punct(byte as char));
                cursor += 1;
            }
        }
    }
    tokens
}

fn daemon_source_inventory(source_root: &Path) -> BTreeSet<PathBuf> {
    fn visit(directory: &Path, source_root: &Path, sources: &mut BTreeSet<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap_or_else(|error| {
            panic!(
                "daemon source directory {} must be readable: {error}",
                directory.display()
            )
        }) {
            let entry = entry.expect("daemon source directory entries must be readable");
            let path = entry.path();
            if path.is_dir() {
                visit(&path, source_root, sources);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.insert(
                    path.strip_prefix(source_root)
                        .expect("daemon source must remain below src")
                        .to_owned(),
                );
            }
        }
    }

    let mut sources = BTreeSet::new();
    visit(source_root, source_root, &mut sources);
    sources
}

fn assert_frozen_daemon_source_inventory(source_root: &Path) {
    assert!(
        !source_root
            .parent()
            .expect("daemon source root must have a package directory")
            .join("build.rs")
            .exists(),
        "PAC-063 rejects an unclassified daemon build-script source input"
    );
    let expected = [
        "blocking.rs",
        "development_v2.rs",
        "dispatch.rs",
        "endpoint.rs",
        "execution.rs",
        "lib.rs",
        "main.rs",
        "native_execution.rs",
        "observability.rs",
        "peer.rs",
        "production.rs",
        "read_service.rs",
        "registry.rs",
        "runtime.rs",
        "runtime_workspace.rs",
        "scheduler.rs",
        "server.rs",
        "worker.rs",
        "workspace.rs",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<BTreeSet<_>>();
    let actual = daemon_source_inventory(source_root);
    assert_eq!(
        actual, expected,
        "PAC-063 source inventory is frozen: every daemon .rs input under src must be explicitly reviewed"
    );

    for relative_source in &actual {
        let source_path = source_root.join(relative_source);
        let parent = source_path
            .parent()
            .expect("daemon source must have a parent");
        let tokens = rust_tokens(&fs::read_to_string(&source_path).unwrap_or_else(|error| {
            panic!(
                "daemon source {} must be readable: {error}",
                source_path.display()
            )
        }));
        for index in 0..tokens.len() {
            let RustTokenV1::Ident(keyword) = &tokens[index] else {
                continue;
            };
            if keyword == "include"
                && matches!(tokens.get(index + 1), Some(RustTokenV1::Punct('!')))
            {
                let input = match (
                    tokens.get(index + 1),
                    tokens.get(index + 2),
                    tokens.get(index + 3),
                    tokens.get(index + 4),
                ) {
                    (
                        Some(RustTokenV1::Punct('!')),
                        Some(RustTokenV1::Punct('(')),
                        Some(RustTokenV1::StringLiteral(path)),
                        Some(RustTokenV1::Punct(')')),
                    )
                    | (
                        Some(RustTokenV1::Punct('!')),
                        Some(RustTokenV1::Punct('{')),
                        Some(RustTokenV1::StringLiteral(path)),
                        Some(RustTokenV1::Punct('}')),
                    )
                    | (
                        Some(RustTokenV1::Punct('!')),
                        Some(RustTokenV1::Punct('[')),
                        Some(RustTokenV1::StringLiteral(path)),
                        Some(RustTokenV1::Punct(']')),
                    ) => parent.join(path),
                    _ => panic!(
                        "daemon source {} has an unclassified include! input; generated and macro-composed inputs require an explicit inventory review",
                        source_path.display()
                    ),
                };
                let relative_input = input.strip_prefix(source_root).unwrap_or_else(|_| {
                    panic!(
                        "daemon source {} includes unclassified external input {}",
                        source_path.display(),
                        input.display()
                    )
                });
                assert!(
                    actual.contains(relative_input),
                    "daemon source {} includes unclassified source input {}",
                    source_path.display(),
                    input.display()
                );
            }
            if keyword == "mod"
                && matches!(tokens.get(index + 1), Some(RustTokenV1::Ident(_)))
                && matches!(tokens.get(index + 2), Some(RustTokenV1::Punct(';')))
            {
                let module = match &tokens[index + 1] {
                    RustTokenV1::Ident(module) => module,
                    _ => unreachable!(),
                };
                let path_attribute = tokens[index.saturating_sub(32)..index]
                    .windows(3)
                    .rev()
                    .find_map(|window| match window {
                        [
                            RustTokenV1::Ident(attribute),
                            RustTokenV1::Punct('='),
                            RustTokenV1::StringLiteral(path),
                        ] if attribute == "path" => Some(path),
                        _ => None,
                    });
                let flat = parent.join(format!("{module}.rs"));
                let nested = parent.join(module).join("mod.rs");
                let input = path_attribute
                    .map(|path| parent.join(path))
                    .unwrap_or_else(|| if flat.is_file() { flat } else { nested });
                let relative_input = input.strip_prefix(source_root).unwrap_or_else(|_| {
                    panic!(
                        "daemon source {} declares unclassified external module input {}",
                        source_path.display(),
                        input.display()
                    )
                });
                assert!(
                    actual.contains(relative_input),
                    "daemon source {} declares module {} with unclassified input {}",
                    source_path.display(),
                    module,
                    input.display()
                );
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImportedPathV1 {
    path: Vec<String>,
}

fn path_at(tokens: &[RustTokenV1], start: usize) -> Option<(Vec<String>, usize)> {
    let RustTokenV1::Ident(first) = tokens.get(start)? else {
        return None;
    };
    let mut path = vec![first.clone()];
    let mut cursor = start + 1;
    while matches!(tokens.get(cursor), Some(RustTokenV1::PathSeparator)) {
        let RustTokenV1::Ident(segment) = tokens.get(cursor + 1)? else {
            break;
        };
        path.push(segment.clone());
        cursor += 2;
    }
    Some((path, cursor))
}

fn collect_use_items(
    tokens: &[RustTokenV1],
    cursor: &mut usize,
    prefix: &[String],
    imports: &mut Vec<ImportedPathV1>,
) {
    loop {
        let Some((mut path, next)) = path_at(tokens, *cursor) else {
            panic!("PAC-063 requires syntactically complete use declarations");
        };
        let mut full_path = prefix.to_vec();
        full_path.append(&mut path);
        *cursor = next;

        if matches!(tokens.get(*cursor), Some(RustTokenV1::PathSeparator))
            && matches!(tokens.get(*cursor + 1), Some(RustTokenV1::Punct('{')))
        {
            *cursor += 2;
            collect_use_items(tokens, cursor, &full_path, imports);
        } else {
            if matches!(tokens.get(*cursor), Some(RustTokenV1::PathSeparator))
                && matches!(tokens.get(*cursor + 1), Some(RustTokenV1::Punct('*')))
            {
                *cursor += 2;
            }
            if matches!(tokens.get(*cursor), Some(RustTokenV1::Ident(keyword)) if keyword == "as") {
                assert!(
                    matches!(tokens.get(*cursor + 1), Some(RustTokenV1::Ident(_))),
                    "PAC-063 use alias must have an identifier"
                );
                *cursor += 2;
            }
            imports.push(ImportedPathV1 { path: full_path });
        }

        match tokens.get(*cursor) {
            Some(RustTokenV1::Punct(',')) => {
                *cursor += 1;
                if matches!(tokens.get(*cursor), Some(RustTokenV1::Punct('}'))) {
                    *cursor += 1;
                    return;
                }
            }
            Some(RustTokenV1::Punct('}')) => {
                *cursor += 1;
                return;
            }
            Some(RustTokenV1::Punct(';')) => {
                *cursor += 1;
                return;
            }
            _ => panic!("PAC-063 requires syntactically complete use declarations"),
        }
    }
}

fn use_items(tokens: &[RustTokenV1]) -> Vec<ImportedPathV1> {
    let mut imports = Vec::new();
    let mut cursor = 0;
    while cursor < tokens.len() {
        if matches!(tokens.get(cursor), Some(RustTokenV1::Ident(keyword)) if keyword == "use") {
            cursor += 1;
            collect_use_items(tokens, &mut cursor, &[], &mut imports);
        } else {
            cursor += 1;
        }
    }
    imports
}

fn is_allowed_nix_peer_credential_path(path: &[String]) -> bool {
    matches!(
        path,
        [root, sys, socket, member]
            if root == "nix"
                && sys == "sys"
                && socket == "socket"
                && member == "getsockopt"
    ) || matches!(
        path,
        [root, sys, socket, sockopt, member]
            if root == "nix"
                && sys == "sys"
                && socket == "socket"
                && sockopt == "sockopt"
                && member == "PeerCredentials"
    )
}

fn assert_network_path_is_rejected(path: &[String], source: &Path) {
    let rendered = path.join("::");
    let forbidden = match path {
        [root, net, ..] if root == "std" && net == "net" => true,
        [root, os, unix, net, member, ..]
            if root == "std"
                && os == "os"
                && unix == "unix"
                && net == "net"
                && !matches!(
                    member.as_str(),
                    "SocketAddr" | "UnixListener" | "UnixStream"
                ) =>
        {
            true
        }
        [root, sys, socket, ..] if root == "nix" && sys == "sys" && socket == "socket" => {
            !is_allowed_nix_peer_credential_path(path)
        }
        [root, ..] if root == "libc" => true,
        _ => false,
    };
    assert!(
        !forbidden,
        "PAC-063 rejects network listener/client capability {rendered} in {}",
        source.display()
    );
}

fn assert_no_forbidden_daemon_apis(source_root: &Path) {
    const ALLOWED_MACROS: &[&str] = &[
        "assert",
        "assert_eq",
        "assert_ne",
        "debug_assert",
        "eprintln",
        "env",
        "format",
        "json",
        "matches",
        "panic",
        "println",
        "vec",
        "unreachable",
        "write",
    ];

    for relative_source in daemon_source_inventory(source_root) {
        let source_path = source_root.join(&relative_source);
        let tokens = rust_tokens(&fs::read_to_string(&source_path).unwrap_or_else(|error| {
            panic!(
                "daemon source {} must be readable: {error}",
                source_path.display()
            )
        }));

        for import in use_items(&tokens) {
            assert_network_path_is_rejected(&import.path, &source_path);
        }

        for cursor in 0..tokens.len() {
            if let Some((path, next)) = path_at(&tokens, cursor) {
                assert_network_path_is_rejected(&path, &source_path);
                if matches!(tokens.get(next), Some(RustTokenV1::Punct('!')))
                    && path.last().is_some_and(|segment| segment != "if")
                    && matches!(
                        tokens.get(next + 1),
                        Some(RustTokenV1::Punct('(' | '{' | '['))
                    )
                {
                    let macro_name = path
                        .last()
                        .expect("parsed macro path must contain a macro name");
                    assert!(
                        (path.len() == 1 && ALLOWED_MACROS.contains(&macro_name.as_str()))
                            || path.as_slice() == ["serde_json", "json"],
                        "PAC-063 rejects macro-composed capability {} in {}",
                        path.join("::"),
                        source_path.display()
                    );
                }
            }
        }
    }
}

fn assert_network_proof_sentinels() {
    let source = Path::new("<PAC-063 sentinel>");
    for forbidden in [
        "std::net::TcpListener::bind",
        "std::net::UdpSocket::bind",
        "nix::sys::socket::socket",
        "libc::AF_INET6",
        "libc::connect",
    ] {
        let tokens = rust_tokens(forbidden);
        let (path, _) = path_at(&tokens, 0).expect("sentinel must be a path");
        let rejected = std::panic::catch_unwind(|| assert_network_path_is_rejected(&path, source));
        assert!(
            rejected.is_err(),
            "PAC-063 sentinel must reject forbidden capability {forbidden}"
        );
    }

    let ordinary_identifier = rust_tokens("network_listener_name = \"std::net::TcpListener\";");
    assert!(
        path_at(&ordinary_identifier, 0).is_some(),
        "PAC-063 lexer must retain ordinary identifiers"
    );
    let (ordinary_path, _) = path_at(&ordinary_identifier, 0).expect("ordinary identifier path");
    assert_network_path_is_rejected(&ordinary_path, source);
}

#[derive(Debug)]
struct ResolvedPackageV1 {
    name: String,
    features: BTreeSet<String>,
}

fn resolved_daemon_packages() -> (BTreeSet<String>, BTreeMap<String, ResolvedPackageV1>) {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("Cargo metadata must execute");
    assert!(
        output.status.success(),
        "Cargo metadata must resolve the daemon target graph: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Cargo metadata must be valid JSON");
    let root = metadata["resolve"]["root"]
        .as_str()
        .expect("resolved daemon graph must identify its root package");
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolved daemon graph must contain nodes");
    let packages = metadata["packages"]
        .as_array()
        .expect("Cargo metadata must list resolved packages");
    let names = packages
        .iter()
        .filter_map(|package| {
            package["id"]
                .as_str()
                .zip(package["name"].as_str())
                .map(|(id, name)| (id.to_owned(), name.to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let dependencies = nodes
        .iter()
        .filter_map(|node| {
            node["id"].as_str().map(|id| {
                (
                    id.to_owned(),
                    node["dependencies"]
                        .as_array()
                        .expect("resolved daemon node dependencies must be an array")
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let direct_dependencies = dependencies
        .get(root)
        .expect("resolved daemon root package must have a dependency node")
        .iter()
        .map(|id| {
            names
                .get(id)
                .expect("resolved daemon direct dependency must name a package")
                .clone()
        })
        .collect::<BTreeSet<_>>();
    let features = nodes
        .iter()
        .filter_map(|node| {
            node["id"].as_str().map(|id| {
                (
                    id.to_owned(),
                    node["features"]
                        .as_array()
                        .expect("resolved daemon node features must be an array")
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .collect::<BTreeSet<_>>(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let mut pending = vec![root.to_owned()];
    let mut visited = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if visited.insert(id.clone()) {
            pending.extend(
                dependencies
                    .get(&id)
                    .expect("every reachable resolved package must have a node")
                    .iter()
                    .cloned(),
            );
        }
    }
    let resolved_packages = visited
        .into_iter()
        .map(|id| {
            (
                id.clone(),
                ResolvedPackageV1 {
                    name: names
                        .get(&id)
                        .expect("resolved node must name a package")
                        .clone(),
                    features: features
                        .get(&id)
                        .expect("resolved node must declare its selected features")
                        .clone(),
                },
            )
        })
        .collect();
    (direct_dependencies, resolved_packages)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

fn ownership_token(path: &Path) -> SocketIdentity {
    let metadata = fs::symlink_metadata(path).expect("socket fixture must exist");
    SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner_uid: metadata.uid(),
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("fixture permissions must be set");
}

fn stale_socket(path: &Path, mode: u32) -> SocketIdentity {
    let listener = UnixListener::bind(path).expect("stale Unix socket must bind");
    set_mode(path, mode);
    let token = ownership_token(path);
    drop(listener);
    token
}

fn connected_pair(directory: &Path) -> (UnixListener, UnixStream, UnixStream, PathBuf) {
    let path = directory.join("peer.sock");
    let listener = UnixListener::bind(&path).expect("peer listener must bind");
    let client = UnixStream::connect(&path).expect("client must connect to peer listener");
    let (server, _) = listener.accept().expect("listener must accept peer client");
    (listener, client, server, path)
}

#[test]
fn singleton_loser_never_unlinks_the_live_socket() {
    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("first daemon owns endpoint");
    let before = ownership_token(fixture.socket_path());

    assert!(matches!(
        SingletonEndpointV1::acquire(&fixture.paths),
        Err(EndpointErrorV1::AlreadyRunning)
    ));
    assert_eq!(ownership_token(fixture.socket_path()), before);

    owner.shutdown().expect("owner must shut down cleanly");
}

#[test]
fn singleton_loser_with_a_different_socket_never_reaches_that_endpoint() {
    let fixture = RuntimeFixture::new();
    let alternate_socket = fixture.runtime_directory().join("alternate.sock");
    let alternate_paths = fixture
        .paths
        .clone()
        .with_socket_path(&alternate_socket)
        .expect("alternate socket path must be valid");
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("first daemon owns singleton");

    assert!(matches!(
        SingletonEndpointV1::acquire(&alternate_paths),
        Err(EndpointErrorV1::AlreadyRunning)
    ));
    assert!(
        !alternate_socket.exists(),
        "lock loser must not inspect, remove, or bind its different socket"
    );

    owner.shutdown().expect("owner must shut down cleanly");
}
#[test]
fn pac063_daemon_exposes_only_a_private_unix_endpoint_and_no_network_surface() {
    let manifest = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("daemon manifest must be readable");
    assert!(
        manifest.lines().any(|line| line == "autobins = false"),
        "daemon targets must be an explicit, closed-world set"
    );
    let targets = manifest
        .lines()
        .enumerate()
        .filter_map(|(index, line)| (line == "[[bin]]").then_some(index))
        .map(|index| {
            let block = manifest
                .lines()
                .skip(index + 1)
                .take_while(|line| !line.starts_with('['));
            let name = block
                .clone()
                .find_map(|line| line.strip_prefix("name = "))
                .expect("every explicit daemon binary target must name itself");
            let path = manifest
                .lines()
                .skip(index + 1)
                .take_while(|line| !line.starts_with('['))
                .find_map(|line| line.strip_prefix("path = "))
                .expect("every explicit daemon binary target must name its source path");
            (name.to_owned(), path.to_owned())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [("\"podwayd\"".to_owned(), "\"src/main.rs\"".to_owned())]
    );

    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    assert_frozen_daemon_source_inventory(&source_root);
    assert_no_forbidden_daemon_apis(&source_root);
    assert_network_proof_sentinels();
    let (direct_dependencies, resolved_packages) = resolved_daemon_packages();
    assert_eq!(
        direct_dependencies,
        [
            "nix",
            "podway-config",
            "podway-core",
            "podway-git",
            "podway-presets",
            "podway-protocol",
            "podway-service",
            "podway-store",
            "serde",
            "serde_json",
            "sha2",
            "signal-hook",
            "uuid",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        "Cargo's resolved daemon root dependencies are a frozen, closed-world policy"
    );
    for forbidden in ["hyper", "mio", "reqwest", "socket2", "tokio", "ureq"] {
        assert!(
            !resolved_packages
                .values()
                .any(|package| package.name.as_str() == forbidden),
            "resolved daemon dependency graph must not enable network/process package {forbidden}"
        );
    }
    for (package_id, package) in resolved_packages {
        for forbidden in ["net", "tcp", "udp"] {
            assert!(
                !package.features.contains(forbidden),
                "resolved daemon package {package_id} ({}) must not select {forbidden}",
                package.name
            );
        }
        assert!(
            package.name.as_str() == "nix" || !package.features.contains("process"),
            "only the Unix credential dependency may select a process feature ({package_id})"
        );
        assert!(
            package.name.as_str() == "nix" || !package.features.contains("socket"),
            "only the Unix endpoint dependency may select a socket feature ({package_id})"
        );
    }

    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("private Unix endpoint");
    assert!(
        UnixStream::connect(fixture.socket_path()).is_ok(),
        "the daemon transport must be reachable through its Unix-domain endpoint"
    );
    assert_eq!(mode(fixture.runtime_directory()), 0o700);
    assert_eq!(mode(fixture.socket_path()), 0o600);
    owner.shutdown().expect("endpoint shutdown");
}

#[test]
fn verified_stale_socket_is_recovered_after_connect_refusal() {
    let fixture = RuntimeFixture::new();
    fs::create_dir(fixture.runtime_directory()).expect("runtime directory fixture must be created");
    set_mode(fixture.runtime_directory(), 0o700);
    let stale = stale_socket(fixture.socket_path(), 0o600);

    let owner =
        SingletonEndpointV1::acquire(&fixture.paths).expect("stale socket must be recovered");
    let replacement = ownership_token(fixture.socket_path());
    assert_ne!(replacement, stale, "recovery must bind a new socket object");

    owner
        .shutdown()
        .expect("replacement socket must shut down cleanly");
}

#[test]
fn unsafe_runtime_lock_and_socket_paths_fail_closed() {
    let runtime_fixture = RuntimeFixture::new();
    fs::create_dir(runtime_fixture.runtime_directory()).expect("runtime fixture must be created");
    set_mode(runtime_fixture.runtime_directory(), 0o755);
    assert!(matches!(
        SingletonEndpointV1::acquire(&runtime_fixture.paths),
        Err(EndpointErrorV1::UnsafeRuntimeDirectory {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));

    let symlink_fixture = RuntimeFixture::new();
    fs::create_dir(symlink_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(symlink_fixture.runtime_directory(), 0o700);
    fs::write(symlink_fixture.root.join("target"), "not a socket")
        .expect("symlink target fixture must be created");
    symlink(
        symlink_fixture.root.join("target"),
        symlink_fixture.socket_path(),
    )
    .expect("socket symlink fixture must be created");
    assert!(matches!(
        SingletonEndpointV1::acquire(&symlink_fixture.paths),
        Err(EndpointErrorV1::UnsafeSocket {
            violation: EndpointPathViolationV1::Symlink,
            ..
        })
    ));

    let regular_fixture = RuntimeFixture::new();
    fs::create_dir(regular_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(regular_fixture.runtime_directory(), 0o700);
    fs::write(regular_fixture.socket_path(), "not a socket")
        .expect("regular socket-path fixture must be created");
    assert!(matches!(
        SingletonEndpointV1::acquire(&regular_fixture.paths),
        Err(EndpointErrorV1::UnsafeSocket {
            violation: EndpointPathViolationV1::NotSocket,
            ..
        })
    ));

    let socket_mode_fixture = RuntimeFixture::new();
    fs::create_dir(socket_mode_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(socket_mode_fixture.runtime_directory(), 0o700);
    let _stale = stale_socket(socket_mode_fixture.socket_path(), 0o660);
    assert!(matches!(
        SingletonEndpointV1::acquire(&socket_mode_fixture.paths),
        Err(EndpointErrorV1::UnsafeSocket {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));

    let lock_mode_fixture = RuntimeFixture::new();
    fs::create_dir(lock_mode_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(lock_mode_fixture.runtime_directory(), 0o700);
    fs::write(lock_mode_fixture.lock_path(), "lock fixture").expect("lock fixture must be created");
    set_mode(lock_mode_fixture.lock_path(), 0o640);
    assert!(matches!(
        SingletonEndpointV1::acquire(&lock_mode_fixture.paths),
        Err(EndpointErrorV1::UnsafeLockFile {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));
}

#[test]
fn explicit_socket_parent_must_be_an_owner_private_real_directory() {
    let insecure_fixture = RuntimeFixture::new();
    let insecure_parent = insecure_fixture.root.join("insecure-run");
    fs::create_dir(&insecure_parent).expect("insecure parent fixture must be created");
    set_mode(&insecure_parent, 0o755);
    let insecure_paths = insecure_fixture
        .paths
        .clone()
        .with_socket_path(insecure_parent.join("podwayd.sock"))
        .expect("explicit socket path must be structurally valid");
    assert!(matches!(
        SingletonEndpointV1::acquire(&insecure_paths),
        Err(EndpointErrorV1::UnsafeSocketParent {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));

    let symlink_fixture = RuntimeFixture::new();
    let real_parent = symlink_fixture.root.join("real-run");
    let linked_parent = symlink_fixture.root.join("linked-run");
    fs::create_dir(&real_parent).expect("real parent fixture must be created");
    set_mode(&real_parent, 0o700);
    symlink(&real_parent, &linked_parent).expect("socket parent symlink must be created");
    let linked_paths = symlink_fixture
        .paths
        .clone()
        .with_socket_path(linked_parent.join("podwayd.sock"))
        .expect("linked socket path must be structurally valid");
    assert!(matches!(
        SingletonEndpointV1::acquire(&linked_paths),
        Err(EndpointErrorV1::UnsafeSocketParent {
            violation: EndpointPathViolationV1::Symlink,
            ..
        })
    ));
}

#[test]
fn endpoint_creates_private_runtime_lock_and_socket_modes() {
    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("endpoint must be acquired");

    assert_eq!(mode(fixture.runtime_directory()), 0o700);
    assert_eq!(mode(fixture.lock_path()), 0o600);
    assert_eq!(mode(fixture.socket_path()), 0o600);
    assert_eq!(
        owner.socket_ownership_token().owner_uid(),
        geteuid().as_raw(),
        "endpoint ownership must use the daemon effective UID"
    );

    owner.shutdown().expect("owner must shut down cleanly");
}

#[test]
fn replacement_socket_survives_old_guard_shutdown() {
    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("endpoint must be acquired");
    let old_token = owner.socket_ownership_token();

    fs::remove_file(fixture.socket_path())
        .expect("old socket path must be unlinked for replacement");
    let replacement_listener = UnixListener::bind(fixture.socket_path())
        .expect("replacement socket must bind after old path is unlinked");
    set_mode(fixture.socket_path(), 0o600);
    let replacement_token = ownership_token(fixture.socket_path());
    assert_ne!(
        replacement_token,
        SocketIdentity {
            device: old_token.device(),
            inode: old_token.inode(),
            owner_uid: old_token.owner_uid(),
        }
    );

    owner
        .shutdown()
        .expect("old guard shutdown must preserve replacement");
    assert_eq!(ownership_token(fixture.socket_path()), replacement_token);

    drop(replacement_listener);
    fs::remove_file(fixture.socket_path()).expect("replacement fixture socket must be removed");
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "linux"
))]
#[test]
fn native_same_effective_uid_peer_is_accepted() {
    let fixture = RuntimeFixture::new();
    let (listener, client, server, socket_path) = connected_pair(&fixture.root);
    let verifier = PeerUidVerifierV1::for_current_user();
    assert_eq!(
        verifier.expected_uid(),
        geteuid().as_raw(),
        "native peer verification must use the daemon effective UID"
    );

    verifier
        .verify(&server)
        .expect("same-user Unix peer must pass native credential verification");

    drop(client);
    drop(server);
    drop(listener);
    fs::remove_file(socket_path).expect("peer socket fixture must be removed");
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "linux"
)))]
#[test]
fn native_peer_credentials_are_explicitly_unsupported_on_other_targets() {
    let fixture = RuntimeFixture::new();
    let (listener, client, server, socket_path) = connected_pair(&fixture.root);
    let verifier = PeerUidVerifierV1::for_current_user();

    assert!(matches!(
        verifier.verify(&server),
        Err(PeerUidVerificationErrorV1::Credential(
            PeerCredentialErrorV1::UnsupportedPlatform
        ))
    ));

    drop(client);
    drop(server);
    drop(listener);
    fs::remove_file(socket_path).expect("peer socket fixture must be removed");
}

#[test]
fn injected_mismatch_and_credential_failure_reject_before_frame_reads() {
    let fixture = RuntimeFixture::new();
    let (listener, client, server, socket_path) = connected_pair(&fixture.root);
    let frame_reads = Cell::new(0_u8);
    let mismatch = PeerUidVerifierV1::new(501, FixedPeerCredentialSourceV1::uid(502));

    assert!(matches!(
        mismatch.verify_before_frame(&server, |_| {
            frame_reads.set(frame_reads.get() + 1);
            Ok(())
        }),
        Err(PeerFrameAdmissionErrorV1::Peer(
            PeerUidVerificationErrorV1::UidMismatch {
                expected_uid: 501,
                actual_uid: 502
            }
        ))
    ));
    assert_eq!(
        frame_reads.get(),
        0,
        "mismatched peer must not reach the frame reader"
    );

    let failure = PeerUidVerifierV1::new(
        501,
        FixedPeerCredentialSourceV1::failure(PeerCredentialErrorV1::UnsupportedPlatform),
    );
    assert!(matches!(
        failure.verify_before_frame(&server, |_| {
            frame_reads.set(frame_reads.get() + 1);
            Ok(())
        }),
        Err(PeerFrameAdmissionErrorV1::Peer(
            PeerUidVerificationErrorV1::Credential(PeerCredentialErrorV1::UnsupportedPlatform)
        ))
    ));
    assert_eq!(
        frame_reads.get(),
        0,
        "credential failure must not reach the frame reader"
    );

    drop(client);
    drop(server);
    drop(listener);
    fs::remove_file(socket_path).expect("peer socket fixture must be removed");
}
