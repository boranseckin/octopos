#![no_std]
#![no_main]

use user::*;

fn type_char(t: InodeType) -> char {
    match t {
        InodeType::File => 'f',
        InodeType::Directory => 'd',
        InodeType::Device => 'D',
        InodeType::Free => '?',
    }
}

fn ls(path: &str) {
    let Ok(fd) = open(path, OpenFlag::READ_ONLY) else {
        eprintln!("ls: cannot open {}", path);
        return;
    };

    let mut stat = Stat::default();
    if fstat(fd, &mut stat) == usize::MAX {
        eprintln!("ls: cannot stat {}", path);
        close(fd);
        return;
    }

    match stat.r#type {
        InodeType::Free => {}
        InodeType::Directory => {
            let mut buf = [0u8; size_of::<Directory>()];
            while read(fd, &mut buf) == buf.len() {
                let dir: &Directory = unsafe { &*(buf.as_ptr() as *const Directory) };

                if dir.inum == 0 {
                    continue; // empty slot
                }

                let mut full_path = [0u8; MAXPATH];

                let mut path_len = path.len();
                let name_len = dir
                    .name
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(dir.name.len());

                full_path[..path_len].copy_from_slice(path.as_bytes());
                if !path.ends_with('/') {
                    full_path[path_len] = b'/';
                    path_len += 1;
                }
                full_path[path_len..path_len + name_len].copy_from_slice(&dir.name[..name_len]);

                let file_name = unsafe { str_from_cstr(&dir.name).expect("ls: malformed path") };
                let file_path = unsafe { str_from_cstr(&full_path).expect("ls: malformed path") };

                let Ok(file_fd) = open(file_path, OpenFlag::READ_ONLY) else {
                    eprintln!("ls: cannot open {}", file_name);
                    continue;
                };

                let mut file_stat = Stat::default();
                if fstat(file_fd, &mut file_stat) == usize::MAX {
                    eprintln!("ls: cannot stat {}", file_name);
                    close(file_fd);
                    continue;
                }

                println!(
                    "{} {:>4} {:>8} {}",
                    type_char(file_stat.r#type),
                    file_stat.ino,
                    file_stat.size,
                    file_name,
                );

                close(file_fd);
            }
        }
        InodeType::File | InodeType::Device => {
            println!(
                "{} {:>4} {:>8} {}",
                type_char(stat.r#type),
                stat.ino,
                stat.size,
                path
            );
        }
    }

    close(fd);
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        ls(".");
    } else {
        args.args_as_str().for_each(ls);
    }
}
