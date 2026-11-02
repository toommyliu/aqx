pub fn read(prompt: &str) -> Result<String, String> {
    eprint!("{prompt}");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

pub fn confirm(question: &str) -> Result<bool, String> {
    Ok(read(&format!("{question} [y/N] "))?.trim().eq_ignore_ascii_case("y"))
}

pub fn read_hidden(prompt: &str) -> Result<String, String> {
    let fd = libc::STDIN_FILENO;
    let mut shown = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(fd, &mut shown) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    ctrlc::set_handler(move || {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &shown) };
        eprintln!();
        std::process::exit(130);
    })
    .map_err(|e| e.to_string())?;
    let mut hidden = shown;
    hidden.c_lflag &= !libc::ECHO;
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) };
    let line = read(prompt);
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &shown) };
    eprintln!();
    line
}
