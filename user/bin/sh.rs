#![no_std]
#![no_main]

use core::iter::Peekable;
use core::str::Chars;

use user::*;

const MAXLINE: usize = 128;
const MAXARGS: usize = 16;
const MAXNODES: usize = 16;
const MAXARGV: usize = 32;

#[derive(Debug, Clone)]
enum CommandType<'a> {
    /// A single command with arguments, e.g. `ls -l`
    Exec { argv_start: usize, argc: usize },

    /// A command with output redirection, e.g. `ls > out.txt`
    Redirect {
        cmd: usize,
        file: &'a str,
        mode: usize,
        fd: Fd,
    },

    /// Two commands connected by a pipe, e.g. `ls | grep foo`
    Pipe { left: usize, right: usize },

    /// Two commands separated by `;`, e.g. `ls; echo done`
    List { left: usize, right: usize },

    /// A command followed by `&`, e.g. `sleep 10 &`
    Background { cmd: usize },
}

#[derive(Debug, Clone)]
struct Arena<'a> {
    nodes: [Option<CommandType<'a>>; MAXNODES],
    node_count: usize,
    argv: [&'a str; MAXARGV],
    argv_count: usize,
}

impl<'a> Arena<'a> {
    fn alloc_argv(&mut self, args: &[&'a str]) -> (usize, usize) {
        let start = self.argv_count;
        for &arg in args {
            self.argv[self.argv_count] = arg;
            self.argv_count += 1;
            if self.argv_count >= self.argv.len() {
                panic!("argv arena overflow");
            }
        }
        (start, args.len())
    }
}

#[derive(Debug)]
struct Tokenizer<'a> {
    input: &'a str,
    iter: Peekable<Chars<'a>>,
    cursor: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            iter: input.chars().peekable(),
            cursor: 0,
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.iter.peek().copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.iter.next()?;
        self.cursor += c.len_utf8();
        Some(c)
    }

    fn next_token(&mut self) -> Option<&'a str> {
        while let Some(c) = self.peek()
            && c.is_whitespace()
        {
            self.next();
        }

        let start = self.cursor;

        if let Some(c) = self.next() {
            match c {
                // single-char tokens
                '|' | '(' | ')' | ';' | '&' | '<' => Some(&self.input[start..self.cursor]),

                // double-char token ">>"
                '>' => {
                    if self.peek() == Some('>') {
                        self.next();
                    }
                    Some(&self.input[start..self.cursor])
                }

                // word token
                _ => {
                    // read until next whitespace or special char
                    while let Some(c) = self.peek() {
                        if c.is_whitespace() || "|();&<>".contains(c) {
                            break;
                        }

                        self.next();
                    }

                    Some(&self.input[start..self.cursor])
                }
            }
        } else {
            None
        }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_token()
    }
}

/// line     = pipe ( '&' )* ( ';' line )?
/// pipe     = exec ( '|' pipe )?
/// exec     = ( '(' line ')' | arg* ) redirs
/// redirs   = ( '<' | '>' | '>>' ) filename )*
#[derive(Debug)]
struct Parser<'a> {
    tokens: Peekable<Tokenizer<'a>>,
    arena: Arena<'a>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            tokens: Tokenizer::new(input).peekable(),
            arena: Arena {
                nodes: [const { None }; MAXNODES],
                node_count: 0,
                argv: [""; MAXARGV],
                argv_count: 0,
            },
        }
    }

    fn alloc_cmd(&mut self, cmd: CommandType<'a>) -> usize {
        let idx = self.arena.node_count;
        self.arena.nodes[idx] = Some(cmd);
        self.arena.node_count += 1;

        if self.arena.node_count >= self.arena.nodes.len() {
            panic!("sh: arena overflow");
        }

        idx
    }

    fn peek_token(&mut self) -> Option<&'a str> {
        self.tokens.peek().copied()
    }

    fn next_token(&mut self) -> Option<&'a str> {
        self.tokens.next()
    }

    fn parse_line(&mut self) -> Option<CommandType<'a>> {
        let mut cmd = self.parse_pipe()?;

        if self.peek_token() == Some("&") {
            self.next_token();
            cmd = CommandType::Background {
                cmd: self.alloc_cmd(cmd),
            };
        }

        if self.peek_token() == Some(";") {
            self.next_token();
            if let Some(right) = self.parse_line() {
                return Some(CommandType::List {
                    left: self.alloc_cmd(cmd),
                    right: self.alloc_cmd(right),
                });
            };
        }

        Some(cmd)
    }

    fn parse_pipe(&mut self) -> Option<CommandType<'a>> {
        let left = self.parse_exec()?;

        if self.peek_token() == Some("|") {
            self.next_token();
            let right = self.parse_pipe()?;
            return Some(CommandType::Pipe {
                left: self.alloc_cmd(left),
                right: self.alloc_cmd(right),
            });
        }

        Some(left)
    }

    fn parse_exec(&mut self) -> Option<CommandType<'a>> {
        // handle sub-shell
        if self.peek_token() == Some("(") {
            self.next_token();
            let cmd = self.parse_line()?;
            if self.peek_token() != Some(")") {
                panic!("sh: expected ')'");
            }
            self.next_token();
            return Some(self.parse_redirects(cmd));
        }

        let mut argv = [""; MAXARGS];
        let mut argc = 0;

        while let Some(token) = self.peek_token() {
            if "|()&;<>>".contains(token) {
                break;
            }

            if argc >= MAXARGS {
                panic!("sh: too many args")
            }

            argv[argc] = token;
            argc += 1;
            self.next_token();
        }

        if argc == 0 {
            return None;
        }

        let (argv_start, argc) = self.arena.alloc_argv(&argv[..argc]);
        let cmd = CommandType::Exec { argv_start, argc };

        Some(self.parse_redirects(cmd))
    }

    fn parse_redirects(&mut self, mut cmd: CommandType<'a>) -> CommandType<'a> {
        while let Some(token) = self.peek_token() {
            match token {
                "<" => {
                    self.next_token();
                    let file = self.next_token().expect("file");
                    let inner = self.alloc_cmd(cmd);
                    cmd = CommandType::Redirect {
                        cmd: inner,
                        file,
                        mode: OpenFlag::READ_ONLY,
                        fd: Fd::STDIN,
                    };
                }
                ">" => {
                    self.next_token();
                    let file = self.next_token().expect("file");
                    let inner = self.alloc_cmd(cmd);
                    cmd = CommandType::Redirect {
                        cmd: inner,
                        file,
                        mode: OpenFlag::WRITE_ONLY | OpenFlag::CREATE | OpenFlag::TRUNCATE,
                        fd: Fd::STDOUT,
                    };
                }
                ">>" => {
                    self.next_token();
                    let file = self.next_token().expect("file");
                    let inner = self.alloc_cmd(cmd);
                    cmd = CommandType::Redirect {
                        cmd: inner,
                        file,
                        mode: OpenFlag::WRITE_ONLY | OpenFlag::CREATE,
                        fd: Fd::STDOUT,
                    };
                }
                _ => break,
            }
        }

        cmd
    }

    fn parse(&mut self) -> Option<CommandType<'a>> {
        self.parse_line()
    }
}

impl<'a> Iterator for Parser<'a> {
    type Item = CommandType<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.parse()
    }
}

fn run_cmd(cmd: CommandType, arena: &mut Arena) -> ! {
    match cmd {
        CommandType::Exec { argv_start, argc } => {
            exec_cmd(
                arena.argv[argv_start],
                &arena.argv[argv_start..argv_start + argc],
            );
        }
        CommandType::Redirect {
            cmd,
            file,
            mode,
            fd,
        } => {
            close(fd).unwrap();
            if open(file, mode).is_err() {
                eprintln!("sh: cannot open {}", file);
                exit(1);
            }

            // now fd 0 or 1 points to the file
            let inner = arena.nodes[cmd].take().expect("sh: inner cmd");
            run_cmd(inner, arena);
        }
        CommandType::Pipe { left, right } => {
            let (read_fd, write_fd) = pipe().expect("sh: pipe failed");

            if fork().expect("sh: fork failed") == 0 {
                // won't read from pipe, close read side
                close(read_fd).unwrap();
                // close STDOUT to free fd 1
                close(Fd::STDOUT).unwrap();
                // write_fd is now copied to lowest free fd (1)
                dup(write_fd).unwrap();
                // won't need the original fd anymore
                close(write_fd).unwrap();

                // exec writes to STDOUT will go to fd 1, which is now the pipe
                let left_cmd = arena.nodes[left].take().expect("sh: left cmd");
                run_cmd(left_cmd, arena)
            }

            if fork().expect("sh: fork failed") == 0 {
                // won't write to pipe, close write side
                close(write_fd).unwrap();
                // close STDIN to free fd 0
                close(Fd::STDIN).unwrap();
                // read_fd is now copied to lowest free fd (0)
                dup(read_fd).unwrap();
                // won't need the original fd anymore
                close(read_fd).unwrap();

                // exec reads from STDIN will come from fd 0, which is now the pipe
                let right_cmd = arena.nodes[right].take().expect("sh: right cmd");
                run_cmd(right_cmd, arena)
            }

            // parent close pipes, wait for left and right
            close(read_fd).unwrap();
            close(write_fd).unwrap();
            wait(&mut 0).unwrap();
            wait(&mut 0).unwrap();
            exit(0);
        }
        CommandType::List { left, right } => {
            let pid = fork().expect("sh: fork failed");

            if pid == 0 {
                let left_cmd = arena.nodes[left].take().expect("sh: left cmd");
                run_cmd(left_cmd, arena);
            } else {
                let _ = wait(&mut 0);
                let right_cmd = arena.nodes[right].take().expect("sh: right cmd");
                run_cmd(right_cmd, arena);
            }
        }
        CommandType::Background { cmd } => {
            let pid = fork().expect("sh: fork failed");

            if pid == 0 {
                let inner_cmd = arena.nodes[cmd].take().expect("sh: inner cmd");
                run_cmd(inner_cmd, arena);
            } else {
                // do not wait
                exit(0);
            }
        }
    }
}

fn exec_cmd(cmd: &str, args: &[&str]) -> ! {
    // Build path: "/cmd"
    let mut path_buf = [0u8; 64];
    path_buf[0] = b'/';
    path_buf[1..1 + cmd.len()].copy_from_slice(cmd.as_bytes());
    let path_str = unsafe { core::str::from_utf8_unchecked(&path_buf[..1 + cmd.len()]) };

    exec(path_str, args);
    eprintln!("sh: exec failed {}", cmd);
    exit(1);
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    // ensure that three file descriptors are open
    loop {
        let Ok(fd) = open("console", OpenFlag::READ_WRITE) else {
            exit_with_msg("sh: cannot open console");
        };

        if fd.as_raw() >= 3 {
            let _ = close(fd);
            break;
        }
    }

    let mut buf = [0u8; MAXLINE];

    loop {
        let _ = write(Fd::STDERR, b"$ ");

        let Some(line) = gets(&mut buf) else {
            break; // EOF
        };

        if line.is_empty() {
            continue;
        }

        let mut tokenizer = Tokenizer::new(line);

        match tokenizer.next_token() {
            Some("exit") => exit(0),
            Some("cd") => {
                // chdir must happen from the parent, do not fork
                let path = tokenizer.next_token().unwrap_or("/");
                if let Err(e) = chdir(path) {
                    eprintln!("cd: {} {}", path, e);
                }
            }
            Some(_) => {
                // Fork first, then parse & execute in child
                let pid = fork().expect("sh: fork failed");

                if pid == 0 {
                    let mut parser = Parser::new(line);
                    if let Some(root) = parser.parse() {
                        run_cmd(root, &mut parser.arena);
                    }
                    exit_with_msg("sh: parse failed");
                } else {
                    let _ = wait(&mut 0);
                }
            }
            None => continue,
        }
    }
}
