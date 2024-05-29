use core::ptr;
use core::sync::atomic::Ordering;

use crate::memlayout::UART0;
use crate::printf::PRINTF;
use crate::proc::Cpus;
use crate::spinlock::Mutex;

const fn offset(base_address: usize, reg: usize) -> *mut u8 {
    unsafe { (base_address as *mut u8).add(reg) }
}

const RHR: usize = 0;
const THR: usize = 0;
const IER: usize = 1;
const IER_RX_ENABLE: u8 = 1 << 0;
const IER_TX_ENABLE: u8 = 1 << 0;
const FCR: usize = 2;
const FCR_FIFO_ENABLE: u8 = 1 << 0;
const FCR_FIFO_CLEAR: u8 = 3 << 1;
const _ISR: usize = 2;
const LCR: usize = 3;
const LCR_EIGHT_BITS: u8 = 3 << 0;
const LCR_BAUD_LATCH: u8 = 1 << 7;
const LSR: usize = 5;
const _LSR_RX_READY: u8 = 1 << 0;
const LSR_TX_IDLE: u8 = 1 << 5;

pub static UART: Mutex<Uart> = Mutex::new(Uart::new(UART0), "uart");

const UART_TX_BUF_SIZE: usize = 32;

pub struct Uart {
    base_address: usize,
    tx_buf: [u8; UART_TX_BUF_SIZE],
    tx_w: usize,
    tx_r: usize,
}

impl Uart {
    pub const fn new(base_address: usize) -> Self {
        Self {
            base_address,
            tx_buf: [0; UART_TX_BUF_SIZE],
            tx_w: 0,
            tx_r: 0,
        }
    }

    fn read(&self, reg: usize) -> u8 {
        unsafe { ptr::read_volatile(offset(self.base_address, reg)) }
    }

    fn write(&mut self, reg: usize, value: u8) {
        unsafe { ptr::write_volatile(offset(self.base_address, reg), value) }
    }

    pub fn init(&mut self) {
        // Disable interrupts
        self.write(IER, 0x00);

        // Special mode to set baud rate
        self.write(LCR, LCR_BAUD_LATCH);

        // LSB for baud rate of 38.4K
        self.write(0, 0x03);

        // MSB for baud rate of 38.4K
        self.write(1, 0x00);

        // Leave set-baud mode
        self.write(LCR, LCR_EIGHT_BITS);

        // reset and enable FIFOs
        self.write(FCR, FCR_FIFO_ENABLE | FCR_FIFO_CLEAR);

        // enable transmit and receive interrupts
        self.write(IER, IER_TX_ENABLE | IER_RX_ENABLE);
    }

    fn start(&mut self) {
        loop {
            if self.tx_w == self.tx_r {
                return;
            }

            if (self.read(LSR) & LSR_TX_IDLE) == 0 {
                return;
            }

            let c = self.tx_buf[self.tx_r % UART_TX_BUF_SIZE];
            self.tx_r += 1;

            // TODO: Wakeup

            self.write(THR, c);
        }
    }
}

impl Mutex<Uart> {
    pub fn putc(&self, c: u8) {
        let mut guard = self.lock();

        if PRINTF.is_panicked().load(Ordering::Relaxed) {
            loop {}
        }

        while guard.tx_w == guard.tx_r + UART_TX_BUF_SIZE {
            // TODO: sleep
        }

        let index = guard.tx_w % UART_TX_BUF_SIZE;
        *guard.tx_buf.get_mut(index).unwrap() = c;
        guard.tx_w += 1;

        guard.start();
    }

    pub fn getc(&self) -> Option<u8> {
        // Safety: we are only reading from uart
        let uart = unsafe { self.get_mut_unchecked() };
        if uart.read(LSR) & 0x01 != 0 {
            Some(uart.read(RHR))
        } else {
            None
        }
    }

    pub fn handle_interrupt(&self) {
        while let Some(c) = self.getc() {
            // TODO: console interrupt with c
        }

        self.lock().start();
    }
}

pub fn putc_sync(c: u8) {
    let _intr_lock = Cpus::lock_mycpu();

    // Safety: locked interrupts
    let uart = unsafe { UART.get_mut_unchecked() };

    if PRINTF.is_panicked().load(Ordering::Relaxed) {
        loop {}
    }

    while (uart.read(LSR) & LSR_TX_IDLE) == 0 {}

    uart.write(THR, c);
}

pub unsafe fn init() {
    UART.get_mut_unchecked().init()
}
