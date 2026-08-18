// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

// spell-checker:ignore TAOCP indegree
// spell-checker:ignore (libs) interner

use clap::{Arg, ArgAction, Command};
use rustc_hash::FxHashMap;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::ffi::OsString;
use std::fs::File;
use std::hash::BuildHasher;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use thiserror::Error;
use uucore::display::Quotable;
use uucore::error::{UError, UResult, USimpleError, set_exit_code};
use uucore::{format_usage, show, translate};

/// Compact identifier for an interned token.
type Sym = usize;

/// The bytes that separate two tokens.
///
/// GNU `tsort` splits on space, tab and newline only, which is also what the
/// `--help` text of this implementation documents. In particular a carriage
/// return is part of a token rather than a separator.
const SEPARATORS: [u8; 3] = *b" \t\n";

fn is_separator(byte: u8) -> bool {
    SEPARATORS.contains(&byte)
}

mod options {
    pub const FILE: &str = "file";
}

/// Marks a free slot in [`Interner::slots`]. No token can ever get this symbol,
/// because that would require [`usize::MAX`] distinct tokens.
const EMPTY: Sym = usize::MAX;

/// Number of slots the lookup table starts with. Must be a power of two.
const INITIAL_SLOTS: usize = 1024;

/// Return the bytes `sym` was interned from.
///
/// This is a free function rather than a method so that it can be called while
/// another field of the interner is borrowed mutably.
fn resolve_in<'a>(arena: &'a [u8], ends: &[usize], sym: Sym) -> &'a [u8] {
    let start = if sym == 0 { 0 } else { ends[sym - 1] };
    &arena[start..ends[sym]]
}

/// Maps the tokens read from the input to a [`Sym`] and back.
///
/// Tokens are arbitrary byte strings: the input is not required to be valid
/// UTF-8, and GNU `tsort` passes the bytes through to the output untouched.
///
/// The bytes of each distinct token are copied once into `arena`, and a symbol
/// is an index into `ends`, which records where the token stops in the arena.
/// Lookup goes through an open-addressing table that stores symbols only, so
/// interning a token that was seen before allocates nothing. A `HashMap` keyed
/// by the token bytes would instead have to own a copy of every key, which for
/// input with many distinct tokens costs one allocation per token.
struct Interner {
    /// Bytes of all interned tokens, concatenated.
    arena: Vec<u8>,
    /// `ends[sym]` is where the token of `sym` stops in `arena`.
    ends: Vec<usize>,
    /// Open-addressing table of symbols. Its length is a power of two.
    slots: Vec<Sym>,
}

impl Default for Interner {
    fn default() -> Self {
        Self {
            arena: Vec::new(),
            ends: Vec::new(),
            slots: vec![EMPTY; INITIAL_SLOTS],
        }
    }
}

impl Interner {
    /// Return the symbol for `token`, interning it if it has not been seen.
    fn get_or_intern(&mut self, token: &[u8]) -> Sym {
        // Keep the load factor at one half or below, so probing stays short.
        if (self.ends.len() + 1) * 2 > self.slots.len() {
            self.grow();
        }

        let mask = self.slots.len() - 1;
        let mut idx = Self::hash(token) as usize & mask;
        loop {
            let sym = self.slots[idx];
            if sym == EMPTY {
                let sym = self.ends.len();
                self.arena.extend_from_slice(token);
                self.ends.push(self.arena.len());
                self.slots[idx] = sym;
                return sym;
            }
            if resolve_in(&self.arena, &self.ends, sym) == token {
                return sym;
            }
            idx = (idx + 1) & mask;
        }
    }

    /// Return the bytes that `sym` was interned from.
    fn resolve(&self, sym: Sym) -> &[u8] {
        resolve_in(&self.arena, &self.ends, sym)
    }

    fn hash(token: &[u8]) -> u64 {
        rustc_hash::FxBuildHasher.hash_one(token)
    }

    /// Double the lookup table and reinsert every symbol.
    fn grow(&mut self) {
        let mut slots = vec![EMPTY; self.slots.len() * 2];
        let mask = slots.len() - 1;
        for sym in 0..self.ends.len() {
            let mut idx = Self::hash(resolve_in(&self.arena, &self.ends, sym)) as usize & mask;
            while slots[idx] != EMPTY {
                idx = (idx + 1) & mask;
            }
            slots[idx] = sym;
        }
        self.slots = slots;
    }
}

#[uucore::main]
pub fn uumain(args: impl uucore::Args) -> UResult<()> {
    let matches = uucore::clap_localization::handle_clap_result(uu_app(), args)?;

    let mut inputs = matches
        .get_many::<OsString>(options::FILE)
        .into_iter()
        .flatten();

    let input = inputs.next().expect("default value should be set by clap");

    if let Some(extra) = inputs.next() {
        return Err(USimpleError::new(
            1,
            translate!(
                "tsort-error-extra-operand",
                "operand" => extra.quote()
            ),
        ));
    }

    // Create the directed graph from pairs of tokens in the input data.
    let mut g = Graph::new(input.to_string_lossy().to_string());
    if input == "-" {
        process_input(io::stdin().lock(), &mut g)?;
    } else {
        // some platforms cannot catch this as read error. Needs additional cost by stat
        #[cfg(windows)]
        {
            let input = std::path::Path::new(input);
            if input.is_dir() {
                return Err(TsortError::IsDir(input.to_string_lossy().to_string()).into());
            }
        }
        let file = File::open(input)?;
        // advise the OS we will access the data sequentially if possible
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
        let _ = rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::Sequential);

        let reader = BufReader::new(file);
        process_input(reader, &mut g)?;
    }

    g.run_tsort()
}

pub fn uu_app() -> Command {
    Command::new("tsort")
        .version(uucore::crate_version!())
        .help_template(uucore::localized_help_template("tsort"))
        .override_usage(format_usage(&translate!("tsort-usage")))
        .about(translate!("tsort-about"))
        .infer_long_args(true)
        // no-op flag, needed for POSIX compatibility.
        .arg(
            Arg::new("warn")
                .short('w')
                .action(ArgAction::SetTrue)
                .hide(true),
        )
        .arg(
            Arg::new(options::FILE)
                .hide(true)
                .value_parser(clap::value_parser!(OsString))
                .value_hint(clap::ValueHint::FilePath)
                .default_value("-")
                .num_args(1..)
                .action(ArgAction::Append),
        )
}

#[derive(Debug, Error)]
enum TsortError {
    /// The input file is actually a directory.
    #[error("{input}: {message}", input = .0.maybe_quote(), message = translate!("tsort-error-is-dir"))]
    IsDir(String),

    /// The number of tokens in the input data is odd.
    ///
    /// The length of the list of edges must be even because each edge has two
    /// components: a source node and a target node.
    #[error("{input}: {message}", input = .0.maybe_quote(), message = translate!("tsort-error-odd"))]
    NumTokensOdd(String),

    /// The graph contains a cycle.
    #[error("{input}: {message}", input = .0, message = translate!("tsort-error-loop"))]
    Loop(String),

    /// Wrapper for bubbling up IO errors
    #[error("{0}")]
    IO(#[from] io::Error),
}

impl UError for TsortError {}

/// Report one node that takes part in a cycle, and remember the failure.
///
/// Node names come from the input and are not necessarily valid UTF-8, so they
/// are written as raw bytes. The `show!` macro cannot be used here because it
/// formats through `Display`, which would force the invalid bytes to be
/// replaced.
fn show_loop_node(name: &[u8]) {
    set_exit_code(1);
    let prefix = uucore::util_name();
    // Assembled first so that the line reaches stderr in a single write, the
    // way the `show!` macro emits its own diagnostics.
    let mut line = Vec::with_capacity(prefix.len() + name.len() + 3);
    line.extend_from_slice(prefix.as_bytes());
    line.extend_from_slice(b": ");
    line.extend_from_slice(name);
    line.push(b'\n');
    let _ = io::stderr().lock().write_all(&line);
}

fn process_input<R: BufRead>(mut reader: R, graph: &mut Graph) -> Result<(), TsortError> {
    let mut pending: Option<Sym> = None;
    let mut line = Vec::new();

    // Input is considered to be in the format
    // From1 To1 From2 To2 ...
    // with tokens separated by whitespaces

    loop {
        line.clear();
        // Read raw bytes rather than `String`s: the input may hold any byte
        // sequence, and the tokens have to reach the output unchanged.
        let read = reader.read_until(b'\n', &mut line).map_err(|e| {
            if e.kind() == io::ErrorKind::IsADirectory {
                TsortError::IsDir(graph.name())
            } else {
                e.into()
            }
        })?;
        if read == 0 {
            break;
        }

        for token in line.split(|&b| is_separator(b)).filter(|t| !t.is_empty()) {
            // Intern the token and get a Sym
            let token_sym = graph.interner.get_or_intern(token);

            if let Some(from) = pending.take() {
                graph.add_edge(from, token_sym);
            } else {
                pending = Some(token_sym);
            }
        }
    }
    if pending.is_some() {
        return Err(TsortError::NumTokensOdd(graph.name()));
    }

    Ok(())
}

/// Find the element `x` in `vec` and remove it, returning its index.
fn remove<T>(vec: &mut Vec<T>, x: T) -> Option<usize>
where
    T: PartialEq,
{
    vec.iter().position(|item| *item == x).inspect(|i| {
        vec.remove(*i);
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitedState {
    Opened,
    Closed,
}

#[derive(Default)]
struct Node {
    successor_tokens: Vec<Sym>,
    predecessor_count: usize,
}

impl Node {
    fn add_successor(&mut self, successor_name: Sym) {
        self.successor_tokens.push(successor_name);
    }
}

struct Graph {
    name: String,
    nodes: FxHashMap<Sym, Node>,
    interner: Interner,
}

impl Graph {
    fn new(name: String) -> Self {
        Self {
            name,
            interner: Interner::default(),
            nodes: FxHashMap::default(),
        }
    }

    fn name(&self) -> String {
        self.name.clone()
    }
    fn get_node_name(&self, node_sym: Sym) -> &[u8] {
        self.interner.resolve(node_sym)
    }

    fn add_edge(&mut self, from: Sym, to: Sym) {
        let from_node = self.nodes.entry(from).or_default();
        if from != to {
            from_node.add_successor(to);
            let to_node = self.nodes.entry(to).or_default();
            to_node.predecessor_count += 1;
        }
    }

    fn remove_edge(&mut self, u: Sym, v: Sym) {
        remove(
            &mut self
                .nodes
                .get_mut(&u)
                .expect("node is part of the graph")
                .successor_tokens,
            v,
        );
        self.nodes
            .get_mut(&v)
            .expect("node is part of the graph")
            .predecessor_count -= 1;
    }

    /// Implementation of algorithm T from TAOCP (Don. Knuth), vol. 1.
    fn run_tsort(&mut self) -> UResult<()> {
        let mut independent_nodes_queue: VecDeque<Sym> = self
            .nodes
            .iter()
            .filter_map(|(&sym, node)| {
                if node.predecessor_count == 0 {
                    Some(sym)
                } else {
                    None
                }
            })
            .collect();

        // Sort by resolved string for deterministic output
        independent_nodes_queue
            .make_contiguous()
            .sort_unstable_by(|a, b| self.get_node_name(*a).cmp(self.get_node_name(*b)));
        let mut out = BufWriter::new(io::stdout().lock());
        while !self.nodes.is_empty() {
            let v = self.find_next_node(&mut independent_nodes_queue);
            out.write_all(self.get_node_name(v))?;
            out.write_all(b"\n")?;
            if let Some(node_to_process) = self.nodes.remove(&v) {
                for successor_name in node_to_process.successor_tokens.into_iter().rev() {
                    // we reverse to match GNU tsort order
                    let successor_node = self
                        .nodes
                        .get_mut(&successor_name)
                        .expect("node is part of the graph");
                    successor_node.predecessor_count -= 1;
                    if successor_node.predecessor_count == 0 {
                        independent_nodes_queue.push_back(successor_name);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn indegree(&self, sym: Sym) -> Option<usize> {
        self.nodes.get(&sym).map(|data| data.predecessor_count)
    }

    fn find_next_node(&mut self, frontier: &mut VecDeque<Sym>) -> Sym {
        // If there are no nodes of in-degree zero but there are still
        // un-visited nodes in the graph, then there must be a cycle.
        // We need to find the cycle, display it on stderr, and break it to go on.
        //
        // A cycle is guaranteed to be of length at least two. We break
        // the cycle by deleting an arbitrary edge (the first). That is
        // not necessarily the optimal thing, but it should be enough to
        // continue making progress in the graph traversal, and matches GNU tsort behavior.
        //
        // It is possible that deleting the edge does not actually
        // result in the target node having in-degree zero, so we repeat
        // the process until such a node appears.

        loop {
            match frontier.pop_front() {
                None => self.find_and_break_cycle(frontier),
                Some(v) => return v,
            }
        }
    }

    fn find_and_break_cycle(&mut self, frontier: &mut VecDeque<Sym>) {
        let cycle = self.detect_cycle();
        show!(TsortError::Loop(self.name()));
        for &sym in &cycle {
            show_loop_node(self.get_node_name(sym));
        }
        let u = *cycle.last().expect("cycle must be non-empty");
        let v = cycle[0];
        self.remove_edge(u, v);
        if self.indegree(v).expect("node is part of the graph") == 0 {
            frontier.push_back(v);
        }
    }

    fn detect_cycle(&self) -> Vec<Sym> {
        // Sort by resolved string for deterministic output
        let mut nodes: Vec<_> = self.nodes.keys().copied().collect();
        nodes.sort_unstable_by(|a, b| self.get_node_name(*a).cmp(self.get_node_name(*b)));

        let mut visited = FxHashMap::default();
        let mut stack = Vec::with_capacity(self.nodes.len());
        for &node in &nodes {
            if self.dfs(node, &mut visited, &mut stack) {
                let (loop_entry, _) = stack.pop().expect("loop is not empty");

                return stack
                    .into_iter()
                    .map(|(node, _)| node)
                    .skip_while(|&node| node != loop_entry)
                    .collect();
            }
        }
        unreachable!("detect_cycle is expected to be called only on graphs with cycles");
    }

    fn dfs<'a>(
        &'a self,
        node: Sym,
        visited: &mut FxHashMap<Sym, VisitedState>,
        stack: &mut Vec<(Sym, &'a [Sym])>,
    ) -> bool {
        stack.push((
            node,
            self.nodes
                .get(&node)
                .map_or(&[], |n: &Node| &n.successor_tokens),
        ));
        let state = *visited.entry(node).or_insert(VisitedState::Opened);

        if state == VisitedState::Closed {
            return false;
        }

        while let Some((node, pending_successors)) = stack.pop() {
            let Some((&next_node, pending)) = pending_successors.split_first() else {
                // no more pending successors in the list -> close the node
                visited.insert(node, VisitedState::Closed);
                continue;
            };

            // schedule processing for the pending part of successors for this node
            stack.push((node, pending));

            match visited.entry(next_node) {
                Entry::Vacant(v) => {
                    // first visit of the node
                    v.insert(VisitedState::Opened);
                    stack.push((
                        next_node,
                        self.nodes
                            .get(&next_node)
                            .map_or(&[], |n| &n.successor_tokens),
                    ));
                }
                Entry::Occupied(o) => {
                    if *o.get() == VisitedState::Opened {
                        // We have found a node that was already visited by another iteration => loop completed
                        // the stack may contain unrelated nodes. This allows narrowing the loop down.
                        stack.push((next_node, &[]));
                        return true;
                    }
                }
            }
        }

        false
    }
}
