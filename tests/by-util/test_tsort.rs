// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.
#![allow(clippy::cast_possible_wrap)]

use uutests::at_and_ucmd;
use uutests::new_ucmd;

#[test]
#[cfg(target_os = "linux")]
fn test_tsort_non_utf8_paths() {
    use std::os::unix::ffi::OsStringExt;
    let (at, mut ucmd) = at_and_ucmd!();

    let filename = std::ffi::OsString::from_vec(vec![0xFF, 0xFE]);
    std::fs::write(at.plus(&filename), b"a b\nb c\n").unwrap();

    ucmd.arg(&filename).succeeds().stdout_is("a\nb\nc\n");
}

#[test]
fn test_invalid_arg() {
    new_ucmd!().arg("--definitely-invalid").fails_with_code(1);
}
#[test]
fn test_sort_call_graph() {
    new_ucmd!()
        .arg("call_graph.txt")
        .succeeds()
        .stdout_is_fixture("call_graph.expected");
}

#[test]
fn test_sort_self_loop() {
    new_ucmd!()
        .pipe_in("first first\nfirst second second second")
        .succeeds()
        .stdout_only("first\nsecond\n");
}

#[test]
fn test_sort_floating_nodes() {
    new_ucmd!()
        .pipe_in("d d\nc c\na a\nb b")
        .succeeds()
        .stdout_only("a\nb\nc\nd\n");
}

#[test]
fn test_no_such_file() {
    new_ucmd!()
        .arg("invalid_file_txt")
        .fails()
        .stderr_contains("No such file or directory");
}

#[test]
fn test_version_flag() {
    let version_short = new_ucmd!().arg("-V").succeeds();
    let version_long = new_ucmd!().arg("--version").succeeds();

    assert_eq!(version_short.stdout_str(), version_long.stdout_str());
}

#[test]
fn test_help_flag() {
    let help_short = new_ucmd!().arg("-h").succeeds();
    let help_long = new_ucmd!().arg("--help").succeeds();

    assert_eq!(help_short.stdout_str(), help_long.stdout_str());
}

#[test]
fn test_multiple_arguments() {
    new_ucmd!()
        .arg("call_graph.txt")
        .arg("invalid_file")
        .fails()
        .stderr_contains("extra operand 'invalid_file'");
}

#[test]
fn test_error_on_dir() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.mkdir("tsort_test_dir");
    ucmd.arg("tsort_test_dir")
        .fails()
        .stderr_contains("tsort: tsort_test_dir: read error: Is a directory");
}

#[test]
fn test_split_on_any_whitespace() {
    new_ucmd!()
        .pipe_in("a\nb\n")
        .succeeds()
        .stdout_only("a\nb\n");
}

/// Tokens that are not valid UTF-8 must be passed through unchanged.
///
/// This is the input from GNU issue report #12918: GNU `tsort` sorts the
/// `0xFF` node like any other and writes the byte back out verbatim.
#[test]
fn test_non_utf8_tokens_from_file() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.write_bytes(
        "f",
        b"hrllo\nawuyues\napple\niphone\niphone\na\n\xff\n\xff\n",
    );

    ucmd.arg("f")
        .succeeds()
        .stdout_only_bytes(b"apple\nhrllo\n\xff\niphone\nawuyues\na\n".as_slice());
}

#[test]
fn test_non_utf8_tokens_from_stdin() {
    new_ucmd!()
        .pipe_in(b"\xff \xfe\n".as_slice())
        .succeeds()
        .stdout_only_bytes(b"\xff\n\xfe\n".as_slice());
}

/// Invalid UTF-8 in the middle of an otherwise ASCII token is kept as-is.
#[test]
fn test_non_utf8_inside_token() {
    new_ucmd!()
        .pipe_in(b"a\xff b\xfe\n".as_slice())
        .succeeds()
        .stdout_only_bytes(b"a\xff\nb\xfe\n".as_slice());
}

/// A cycle whose node names are not valid UTF-8 is still reported, with the
/// node names written to stderr as raw bytes.
#[test]
fn test_cycle_with_non_utf8_node() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.write_bytes("f", b"\xff b\nb \xff\n");

    ucmd.arg("f")
        .fails_with_code(1)
        .stdout_is_bytes(b"b\n\xff\n".as_slice())
        .stderr_is_bytes(b"tsort: f: input contains a loop:\ntsort: b\ntsort: \xff\n".as_slice());
}

/// Only space, tab and newline separate tokens, as documented in `--help`.
/// A carriage return is part of the token that contains it.
#[test]
fn test_carriage_return_is_not_a_separator() {
    new_ucmd!()
        .pipe_in(b"a\r b\n".as_slice())
        .succeeds()
        .stdout_only_bytes(b"a\r\nb\n".as_slice());
}

/// A vertical tab does not separate tokens either, so this input holds a
/// single token and the token count is odd.
#[test]
fn test_vertical_tab_is_not_a_separator() {
    new_ucmd!()
        .pipe_in(b"a\x0bb\n".as_slice())
        .fails_with_code(1)
        .stdout_is("")
        .stderr_is(TSORT_ODD_ERROR);
}

/// Same for a form feed.
#[test]
fn test_form_feed_is_not_a_separator() {
    new_ucmd!()
        .pipe_in(b"a\x0cb\n".as_slice())
        .fails_with_code(1)
        .stdout_is("")
        .stderr_is(TSORT_ODD_ERROR);
}

/// Non-ASCII Unicode whitespace is part of a token, not a separator.
#[test]
fn test_unicode_whitespace_is_not_a_separator() {
    new_ucmd!()
        .pipe_in("a\u{a0}b\n".as_bytes())
        .fails_with_code(1)
        .stdout_is("")
        .stderr_is(TSORT_ODD_ERROR);
}

#[test]
fn test_cycle() {
    // The graph looks like:  a --> b <==> c --> d
    new_ucmd!()
        .pipe_in("a b b c c d c b")
        .fails_with_code(1)
        .stdout_is("a\nb\nc\nd\n")
        .stderr_is("tsort: -: input contains a loop:\ntsort: b\ntsort: c\n");
}

#[test]
fn test_two_cycles() {
    // The graph looks like:
    //
    //        a
    //        |
    //        V
    // c <==> b <==> d
    //
    new_ucmd!()
        .pipe_in("a b b c c b b d d b")
        .fails_with_code(1)
        .stdout_is("a\nb\nd\nc\n")
        .stderr_is("tsort: -: input contains a loop:\ntsort: b\ntsort: c\ntsort: -: input contains a loop:\ntsort: b\ntsort: d\n");
}

#[test]
fn test_long_loop_no_stack_overflow() {
    use std::fmt::Write;
    const N: usize = 100_000;
    let mut input = String::new();
    for v in 0..N {
        let next = (v + 1) % N;
        let _ = write!(input, "{v} {next} ");
    }
    new_ucmd!()
        .pipe_in(input)
        .fails_with_code(1)
        .stderr_contains("tsort: -: input contains a loop");
}

#[test]
fn test_loop_for_iterative_dfs_correctness() {
    let input = r"
        A B
        B C
        C B
        C D
        D A
    ";

    new_ucmd!()
        .pipe_in(input)
        .fails_with_code(1)
        .stderr_contains("tsort: -: input contains a loop:\ntsort: B\ntsort: C");
}

const TSORT_LOOP_STDERR: &str = "tsort: f: input contains a loop:\ntsort: s\ntsort: t\n";
const TSORT_LOOP_STDERR_AC: &str = "tsort: f: input contains a loop:\ntsort: a\ntsort: b\ntsort: f: input contains a loop:\ntsort: a\ntsort: c\n";
const TSORT_ODD_ERROR: &str = "tsort: -: input contains an odd number of tokens\n";
const TSORT_EXTRA_OPERAND_ERROR: &str =
    "tsort: extra operand 'g'\nTry 'tsort --help' for more information.\n";

#[test]
fn test_cycle_loop_from_file() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.write("f", "t b\nt s\ns t\n");

    ucmd.arg("f")
        .fails_with_code(1)
        .stdout_is("s\nt\nb\n")
        .stderr_is(TSORT_LOOP_STDERR);
}

#[test]
fn test_cycle_loop_with_extra_node_from_file() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.write("f", "t x\nt s\ns t\n");

    ucmd.arg("f")
        .fails_with_code(1)
        .stdout_is("s\nt\nx\n")
        .stderr_is(TSORT_LOOP_STDERR);
}

#[test]
fn test_cycle_loop_multiple_loops_from_file() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.write("f", "a a\na b\na c\nc a\nb a\n");

    ucmd.arg("f")
        .fails_with_code(1)
        .stdout_is("a\nc\nb\n")
        .stderr_is(TSORT_LOOP_STDERR_AC);
}

#[test]
fn test_posix_graph_examples() {
    new_ucmd!()
        .pipe_in("a b c c d e\ng g\nf g e f\nh h\n")
        .succeeds()
        .stdout_only("a\nc\nd\nh\nb\ne\nf\ng\n");

    new_ucmd!()
        .pipe_in("b a\nd c\nz h x h r h\n")
        .succeeds()
        .stdout_only("b\nd\nr\nx\nz\na\nc\nh\n");
}

#[test]
fn test_linear_tree_graphs() {
    new_ucmd!()
        .pipe_in("a b b c c d d e e f f g\n")
        .succeeds()
        .stdout_only("a\nb\nc\nd\ne\nf\ng\n");

    new_ucmd!()
        .pipe_in("a b b c c d d e e f f g\nc x x y y z\n")
        .succeeds()
        .stdout_only("a\nb\nc\nx\nd\ny\ne\nz\nf\ng\n");

    new_ucmd!()
        .pipe_in("a b b c c d d e e f f g\nc x x y y z\nf r r s s t\n")
        .succeeds()
        .stdout_only("a\nb\nc\nx\nd\ny\ne\nz\nf\nr\ng\ns\nt\n");
}

#[test]
fn test_odd_number_of_tokens() {
    new_ucmd!()
        .pipe_in("a\n")
        .fails_with_code(1)
        .stdout_is("")
        .stderr_is(TSORT_ODD_ERROR);
}

#[test]
fn test_only_one_input_file() {
    let (at, mut ucmd) = at_and_ucmd!();
    at.write("f", "");
    at.write("g", "");

    ucmd.arg("f")
        .arg("g")
        .fails_with_code(1)
        .stdout_is("")
        .stderr_is(TSORT_EXTRA_OPERAND_ERROR);
}
