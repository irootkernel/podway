#![forbid(unsafe_code)]

mod command;

fn main() {
    std::process::exit(command::run());
}
