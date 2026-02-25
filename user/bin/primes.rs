#![no_std]
#![no_main]

use user::*;

// Upper bound for prime counting.
const LIMIT: usize = 30_000_000;

// Must match the -smp value in .cargo/config.toml so we actually saturate all available hardware
// threads.
const NCPU: usize = 4;

// Returns true if `n` is prime via trial division.
//
// We only test odd divisors after eliminating the even case, which halves the number of divisions.
// Checking up to sqrt(n) is sufficient: if n has a factor larger than sqrt(n) it must also have one
// smaller than sqrt(n).
fn is_prime(n: usize) -> bool {
    if n < 2 {
        return false;
    }
    if n == 2 {
        return true;
    }
    if n.is_multiple_of(2) {
        return false;
    }
    let mut d = 3;
    while d * d <= n {
        if n.is_multiple_of(d) {
            return false;
        }
        d += 2;
    }
    true
}

// Count primes in the half-open range [start, end) using plain trial division.
//
// Clamping start to 2 lets the first worker pass its raw start (0 or 1) without special-casing the
// split logic in main.
fn count_primes(start: usize, end: usize) -> usize {
    let mut count = 0;
    let lo = start.max(2);
    for n in lo..end {
        if is_prime(n) {
            count += 1;
        }
    }
    count
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("Range: [2, {})  |  Workers: {}", LIMIT, NCPU);
    println!();

    // Parallel phase
    let range_size = LIMIT / NCPU;

    let mut pipes = [(Fd::STDIN, Fd::STDIN); NCPU]; // placeholder
    for slot in pipes.iter_mut() {
        *slot = pipe().unwrap_or_else(|_| exit_with_msg("primes: pipe failed"));
    }

    println!("[Parallel]");
    let par_start = uptime();

    for i in 0..NCPU {
        // Each worker is responsible for a contiguous slice of the number line.
        let worker_start = i * range_size;
        let worker_end = if i + 1 == NCPU {
            LIMIT
        } else {
            (i + 1) * range_size
        };

        if fork().unwrap_or_else(|_| exit_with_msg("primes: fork failed")) == 0 {
            // Worker: discard every pipe end we don't need, then count and report.
            for (j, &(read_fd, write_fd)) in pipes.iter().enumerate() {
                close(read_fd).expect("primes: close failed");
                if j != i {
                    close(write_fd).expect("primes: close failed");
                }
            }

            let count = count_primes(worker_start, worker_end) as u64;

            write(pipes[i].1, &count.to_le_bytes()).expect("primes: write failed");
            close(pipes[i].1).expect("primes: close failed");

            exit(0);
        }

        println!("  Worker {}: [{}, {})", i, worker_start.max(2), worker_end);
    }

    for &(_, write_fd) in pipes.iter() {
        close(write_fd).expect("primes: close failed");
    }

    let mut status = 0;
    for _ in 0..NCPU {
        wait(&mut status).expect("primes: wait failed");
    }

    let par_elapsed = uptime().saturating_sub(par_start);

    // Sum the partial counts from each worker's pipe.
    let mut par_total = 0;
    for &(read_fd, _) in pipes.iter() {
        let mut buf = [0u8; 8];
        read(read_fd, &mut buf).expect("primes: read failed");
        close(read_fd).expect("primes: close failed");
        par_total += u64::from_le_bytes(buf) as usize;
    }

    println!(
        "  Found {} primes  |  Time: {} ticks",
        par_total, par_elapsed
    );
    println!();

    // Serial phase
    println!("[Serial]");
    let ser_start = uptime();
    let ser_total = count_primes(2, LIMIT);
    let ser_elapsed = uptime().saturating_sub(ser_start);

    println!(
        "  Found {} primes  |  Time: {} ticks",
        ser_total, ser_elapsed
    );
    println!();

    if par_total != ser_total {
        eprintln!(
            "ERROR: parallel ({}) and serial ({}) counts disagree!",
            par_total, ser_total
        );
        exit(1);
    }

    if let Some(speedup) = ser_elapsed.checked_div(par_elapsed) {
        println!("Speedup: ~{}x  (ideal: {}x)", speedup, NCPU);
    } else {
        println!("Parallel time < 1 tick — increase LIMIT for a measurable result.");
    }
}
