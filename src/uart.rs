use core::num::Wrapping;
use core::ptr;
use core::sync::atomic::Ordering;

use crate::memlayout::UART0;
use crate::printf::PRINTF;
use crate::proc::{self, Cpus};
use crate::spinlock::{SpinLock, SpinLockGuard};

// UART control registers are memory-mapped at address UART0.
// http://byterunner.com/16550.html

/// Receive Holding Register (for input bytes)
const RHR: usize = 0;

/// Transmit Holding Register (for output bytes)
const THR: usize = 0;

/// Interrupt Enable Register
const IER: usize = 1;
const IER_RX_ENABLE: u8 = 1 << 0;
const IER_TX_ENABLE: u8 = 1 << 1;

/// FIFO Control Register
const FCR: usize = 2;
const FCR_FIFO_ENABLE: u8 = 1 << 0;
/// Clear the content of the two FIFOs
const FCR_FIFO_CLEAR: u8 = 3 << 1;

/// Interrupt Status Register
const ISR: usize = 2;

/// Line Control Register
const LCR: usize = 3;
const LCR_EIGHT_BITS: u8 = 3;
/// Special mode to set baud rate
const LCR_BAUD_LATCH: u8 = 1 << 7;

/// Line Status Register
const LSR: usize = 5;
/// Input is waiting to be read from RHR
const LSR_RX_READY: u8 = 1 << 0;
/// THR can accept another character to send
const LSR_TX_IDLE: u8 = 1 << 5;

pub static UART: SpinLock<Uart> = SpinLock::new(Uart::new(UART0), "uart");

const UART_TX_BUF_SIZE: usize = 32;

#[derive(Debug)]
pub struct Uart {
    base_address: usize,
    tx_buf: [u8; UART_TX_BUF_SIZE],
    tx_w: Wrapping<usize>,
    tx_r: Wrapping<usize>,
}

impl Uart {
    pub const fn new(base_address: usize) -> Self {
        Self {
            base_address,
            tx_buf: [0; UART_TX_BUF_SIZE],
            tx_w: Wrapping(0),
            tx_r: Wrapping(0),
        }
    }

    /// Read a byte from the given UART register.
    fn read(&self, reg: usize) -> u8 {
        // Safety: reading from memory-mapped UART register
        unsafe { ptr::read_volatile((self.base_address as *mut u8).add(reg)) }
    }

    /// Write a byte to the given UART register.
    fn write(&mut self, reg: usize, value: u8) {
        // Safety: writing to memory-mapped UART register
        unsafe { ptr::write_volatile((self.base_address as *mut u8).add(reg), value) }
    }

    /// Initialize the UART to 38.4K baud, 8 data bits, no parity, one stop bit.
    pub fn init(&mut self) {
        // disable interrupts
        self.write(IER, 0x00);

        // special mode to set baud rate
        self.write(LCR, LCR_BAUD_LATCH);

        // LSB for baud rate of 38.4K
        self.write(0, 0x03);

        // MSB for baud rate of 38.4K
        self.write(1, 0x00);

        // leave set-baud mode
        self.write(LCR, LCR_EIGHT_BITS);

        // reset and enable FIFOs
        self.write(FCR, FCR_FIFO_ENABLE | FCR_FIFO_CLEAR);

        // enable transmit and receive interrupts
        self.write(IER, IER_TX_ENABLE | IER_RX_ENABLE);
    }

    /// Sends character if UART is idle and character is waiting in the transmit buffer.
    ///
    /// Must be called with mutex guard held.
    fn start(&mut self) {
        loop {
            if self.tx_w == self.tx_r {
                // transmit buffer is empty
                self.read(ISR);
                return;
            }

            if (self.read(LSR) & LSR_TX_IDLE) == 0 {
                // UART transmit holding register is full, so we cannot give another byte.
                // It will interrupt again when it's ready for a new byte.
                return;
            }

            let c = *self.tx_buf.get(self.tx_r.0 % UART_TX_BUF_SIZE).unwrap();
            self.tx_r += 1;

            proc::wakeup(&self.tx_r as *const _ as usize);

            self.write(THR, c);
        }
    }
}

impl SpinLock<Uart> {
    /// Adds a character to the output buffer and tell UART to start sending if it isn't already.
    ///
    /// Blocks if the output buffer is full.
    /// Because it may block, it can't be called from interrupts; only suitable for use by `write()`.
    pub fn putc(&self, c: u8) {
        let mut uart = self.lock();

        if PRINTF.is_panicked() {
            #[allow(clippy::empty_loop)]
            loop {}
        }

        // buffer is full, sleep until there is space
        while uart.tx_w == uart.tx_r + Wrapping(UART_TX_BUF_SIZE) {
            uart = proc::sleep(&uart.tx_r as *const _ as usize, uart)
        }

        let index = uart.tx_w.0 % UART_TX_BUF_SIZE;
        *uart.tx_buf.get_mut(index).unwrap() = c;
        uart.tx_w += 1;

        uart.start();
    }

    /// Reads one input character form UART.
    /// Returns None if there is no input waiting.
    pub fn getc(&self) -> Option<u8> {
        // Safety: we are only reading from uart
        let uart = unsafe { self.get_mut_unchecked() };

        if uart.read(LSR) & LSR_RX_READY != 0 {
            Some(uart.read(RHR))
        } else {
            None
        }
    }

    /// Handles a UART interrupt.
    /// It is raised because input has arrived, UART is ready for more output, or both.
    pub fn handle_interrupt(&self) {
        while let Some(c) = self.getc() {
            todo!("console interrupt with c");
        }

        self.lock().start();
    }
}

/// Alternate version of `putc` that doesn't rely on interrupts.
///
/// For use by kernel `printf` and to echo characters.
/// It spins waiting for the UART's output register to be empty.
pub fn putc_sync(c: u8) {
    let _intr_lock = Cpus::lock_mycpu();

    // Safety: locked interrupts
    let uart = unsafe { UART.get_mut_unchecked() };

    if PRINTF.is_panicked() {
        #[allow(clippy::empty_loop)]
        loop {}
    }

    // wait for Transmit Holding Empty to be set in LSR
    while (uart.read(LSR) & LSR_TX_IDLE) == 0 {}

    uart.write(THR, c);
}

pub unsafe fn init() {
    unsafe { UART.get_mut_unchecked().init() }
}
