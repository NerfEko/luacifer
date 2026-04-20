fn main() {
    if let Err(error) = run() {
        eprintln!("evilwm-probe-client: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(mode) = args.next() else {
        print_usage();
        return Ok(());
    };
    if mode == "-h" || mode == "--help" {
        print_usage();
        return Ok(());
    }

    let mut title = String::from("evilwm probe");
    let mut hold_ms = 0_u64;
    let mut namespace = String::from("evilwm-test-panel");

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--title" => {
                title = args.next().ok_or("--title requires a value")?;
            }
            "--hold-ms" => {
                hold_ms = args.next().ok_or("--hold-ms requires a value")?.parse()?;
            }
            "--namespace" => {
                namespace = args.next().ok_or("--namespace requires a value")?;
            }
            "-h" | "--help" => {
                print_usage();
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    match mode.as_str() {
        "xdg-window" => evilwm::probe::xdg_window::run(&title, hold_ms)?,
        "layer-panel" => evilwm::probe::layer_panel::run(&namespace, hold_ms)?,
        "x11-window" => evilwm::probe::x11_window::run(&title, hold_ms)?,
        _ => return Err(format!("unknown mode: {mode}").into()),
    }

    Ok(())
}

fn print_usage() {
    println!(
        "Usage: evilwm-probe-client <xdg-window|layer-panel|x11-window> [--title TEXT] [--namespace NAME] [--hold-ms N]"
    );
}
