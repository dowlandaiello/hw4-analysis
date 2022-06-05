use log::info;
use plotters::{
    chart::ChartBuilder,
    drawing::IntoDrawingArea,
    element::PathElement,
    prelude::{BitMapBackend, IntoFont},
    series::LineSeries,
    style::{colors, Color},
};
use regex::Regex;
use std::{
    env::args,
    process::{Command, Output},
};

/// The name of the command used to benchmark the HTTP server
const BENCH_CMD: &'static str = "httperf";

/// The number of times the tester should re-run a route for consistency
const SAMPLES: usize = 10;

/// The default tests to run
const DEFAULT_TESTS: [TestKind; 3] = [
    TestKind::Latency,
    TestKind::ThroughputBytes,
    TestKind::ThroughputReq,
];

/// The names of the files that the results from the test should be written to, by default
const OUT_FILES: [&'static str; 3] = [
    "multisampled_latency.png",
    "multisampled_throughput_bytes.png",
    "multisampled_throughput_requests.png",
];

/// 3 tests are available:
/// - a test that tests the average time for a response to be received
/// - a test that determines the maximum number of bytes serviceable
/// - a test that determines the maximum number of requests serviceable
#[derive(Clone, Copy)]
enum TestKind {
    Latency,
    ThroughputBytes,
    ThroughputReq,
}

/// A test has multiple data dependencies:
/// - The address of the server it is testing on
/// - The port of the server it should test against
/// - The kind of the test
/// - The dictionary to use for the test
/// - The name of the file the test's results should be written to
struct Test<'a> {
    server_addr: &'a str,
    server_port: &'a u16,
    dict: Vec<String>,
    kind: TestKind,
    out_file: &'a str,
}

/// A script that starts successive httperf instances with varying query sizes,
/// where the address of the server is the first cli arg, the port of the
/// server the second, and the dictionary of query words are the last arguments.
fn main() {
    // For prefixing logs with severity labels
    env_logger::init();

    let mut args = args();

    // Handle no arguments, which means usage
    if args.len() == 1 {
        println!(
            "./{} <server_addr> <port_number> <query_word1> <query_word2> ...",
            args.next().expect("No program name")
        );

        return;
    }

    let server_addr = args.nth(1).expect("Missing server address.");

    // TCP port numbers are 2^16 max
    let port = args
        .next()
        .expect("Missing port number.")
        .parse::<u16>()
        .expect("Invalid port number.");

    let dictionary = args.collect::<Vec<String>>();

    for i in 0..3 {
        do_test(Test {
            server_addr: server_addr.as_str(),
            server_port: &port,
            dict: dictionary.clone(),
            kind: DEFAULT_TESTS[i],
            out_file: OUT_FILES[i],
        })
    }
}

/// Performs the indicated test, crashing the program if an error occurs.
fn do_test<'a>(test: Test<'a>) {
    // Convenient aliases for the execution of the test
    let Test {
        server_addr,
        server_port: port,
        dict: dictionary,
        kind,
        out_file,
    } = test;

    let throughput_tester = Box::new(move |mut cmd: Command| {
                    cmd.arg("--num-conns");
                    cmd.arg(SAMPLES.to_string());

                    return cmd;
                });

    let (y_title, regex_expr, mut query_args): (&str, &str, Box<dyn FnMut(Command) -> Command>) =
        match kind {
            TestKind::Latency => (
                "Avg. Response Latency (ms)",
                r"Connection time.*avg (\S+) max",
                Box::new(move |mut cmd: Command| {
                    cmd.arg("--num-calls");
                    cmd.arg(SAMPLES.to_string());

                    return cmd;
                }),
            ),
            TestKind::ThroughputReq => (
                "Max. Throughput (req./sec.)",
                r"Request rate: (\S+) req",
                throughput_tester,
            ),
            TestKind::ThroughputBytes => (
                "Max. Throughput (KB/sec.)",
                r"Net I/O: (\S+) ",
                throughput_tester,
            ),
        };

    // Where average response times from each subsequent test is put
    let mut buf: Vec<(f32, f32)> = Vec::new();

    for i in 0..dictionary.len() {
        // Stores a + separated list of words from the n words from the provided
        // dictionary currently being tested
        let query: String = (&dictionary[0..=i])
            .iter()
            .cloned()
            .reduce(|a, b| format!("{a}+{b}"))
            .expect("Couldn't build query.");
        let query_url = format!("/query?terms={}", query);

        info!(
            "Running query #{}: http://{}:{}{}",
            i, server_addr, port, query_url
        );

        let mut test_cmd = Command::new(BENCH_CMD);
        test_cmd
            .arg("--server")
            .arg(&server_addr)
            .arg("--port")
            .arg(port.to_string())
            .arg("--uri")
            .arg(query_url);

        // Apply arguments specific to the test type
        test_cmd = query_args(test_cmd);

        let test_out = test_cmd.output().expect("Failed to execute test.");

        // The average number of seconds the server took to process the query
        let rate = parse_output(test_out, regex_expr);
        buf.push((query.len() as f32, rate));

        info!("Query #{} finished: avg. response time - {}ms", i, rate);
    }

    let mut max_x = buf.iter().map(|(x, _)| x).cloned().collect::<Vec<f32>>();
    max_x.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mut max_y = buf.iter().map(|(_, y)| y).cloned().collect::<Vec<f32>>();
    max_y.sort_by(|a, b| a.partial_cmp(b).unwrap());

    // Used for building axis extents
    let min_x = max_x[0];
    let min_y = max_y[0];
    let max_x = *max_x.last().unwrap();
    let max_y = *max_y.last().unwrap();

    // This program plots the results it finds to a plot using plotters
    let canvas = BitMapBackend::new(out_file, (640, 480)).into_drawing_area();
    canvas.fill(&colors::WHITE).expect("Couldn't fill plot.");

    let canvas = canvas.margin(10, 10, 10, 10);
    let mut plt = ChartBuilder::on(&canvas)
        .caption(
            format!("Index Query Word Length vs {y_title}"),
            ("sans-serif", 16).into_font(),
        )
        .x_label_area_size(20)
        .y_label_area_size(40)
        .build_cartesian_2d(min_x..max_x, min_y..max_y)
        .expect("Couldn't make plot canvas.");
    plt.configure_mesh().draw().expect("Couldn't draw plot.");

    plt.draw_series(LineSeries::new(buf, &colors::RED))
        .expect("Couldn't draw series.")
        .label(format!("{y_title} n={SAMPLES}"))
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], &colors::RED));

    plt.configure_series_labels()
        .background_style(&colors::WHITE.mix(0.8))
        .border_style(&colors::BLACK)
        .draw()
        .expect("Couldn't draw plot.");
}

/// Returns the number of requests per second from the httper test, followed
/// by the complexity of the queries issued.
fn parse_output<'a>(output: Output, regex: &'a str) -> f32 {
    // Use a regex to capture the
    // `Reply rate [replies/s]: min 0.0 avg 0.0 max 0.0 stddev 0.0 (0 samples)`
    // line of the output
    let raw_out = String::from_utf8(output.stdout).expect("Test had no output.");
    let str_rate = Regex::new(regex)
        .expect("Could not build regex.")
        .captures(raw_out.as_ref())
        .and_then(|capture| capture.get(1))
        .map(|r_match| r_match.as_str())
        .expect("No reply rate in test output.");

    // The reply rate is in decimal format
    str_rate
        .parse::<f32>()
        .expect("Reply rate from test was not a valid integer.")
}
