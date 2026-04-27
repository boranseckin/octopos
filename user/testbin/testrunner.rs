#![no_std]
#![no_main]

use user::*;

const TESTS: &[&str] = &[
    "/_fs", "/_pipe", "/_proc", "/_fd", "/_sbrk", "/_cow", "/_net",
];

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("running {} tests\n", TESTS.len());

    let mut passed = 0;
    let mut failed = 0;

    for name in TESTS {
        print!("test {} ... ", &name[2..]);

        if fork().expect("fork") == 0 {
            exec(name, &[&name[2..]]);
            unreachable!("exec failed");
        }

        let mut code = 0;
        wait(&mut code).expect("wait failed");

        if code == 0 {
            println!("ok");
            passed += 1;
        } else {
            println!("FAILED");
            failed += 1;
        }
    }

    println!(
        "\ntest result: {}. {} passed; {} failed",
        if failed == 0 { "ok" } else { "FAILED" },
        passed,
        failed,
    );

    poweroff(if failed == 0 { 0 } else { 1 });
}
