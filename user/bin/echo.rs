#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    for (i, arg) in args.args_as_str().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", arg);
    }
    println!();
}
