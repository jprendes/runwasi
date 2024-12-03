use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use clone3::Clone3;
use error::{IntoAnyhow as _, IpcError};
use serde::{Deserialize, Serialize};

use self::error::IpcResult;
use self::pipe::{bidir_pipe, BidirPipe, EofOk as _, ReadSerde as _, WriteSerde as _};

mod error;
mod pipe;

pub struct Zygote {
    pipe: Mutex<BidirPipe>,
}

impl Zygote {
    pub fn global() -> &'static Self {
        static ZYGOTE: LazyLock<Zygote> = LazyLock::new(Zygote::new);
        &*ZYGOTE
    }

    pub fn new() -> Self {
        let (child_pipe, parent_pipe) = bidir_pipe();
        let mut clone3 = Clone3::default();
        clone3.exit_signal(libc::SIGCHLD as _);
        match unsafe { clone3.call() }.unwrap() {
            0 => {
                drop(parent_pipe);
                match zygote_main(child_pipe).eof_ok() {
                    Ok(()) => std::process::exit(0),
                    Err(err) => {
                        log::warn!("zygote exited: {err}");
                        std::process::exit(1);
                    }
                }
            }
            _child_pid => {
                drop(child_pipe);
                return Self {
                    pipe: Mutex::new(parent_pipe),
                };
            }
        }
    }

    pub fn run<
        Args: Serialize + for<'a> Deserialize<'a>,
        Res: Serialize + for<'a> Deserialize<'a>,
        Err: Into<IpcError>,
    >(
        &self,
        f: fn(Args) -> Result<Res, Err>,
        args: &Args,
    ) -> Result<Res> {
        let mut pipe = self.pipe.lock().unwrap();
        let runner = runner::<Args, Res, Err> as usize;
        let f = f as usize;
        pipe.write_serde(&[f, runner])?;
        pipe.write_serde(&args)?;
        let res: IpcResult<Res> = pipe.read_serde()?;
        Ok(res.into_anyhow()?)
    }
}

fn runner<
    Args: Serialize + for<'a> Deserialize<'a>,
    Res: Serialize + for<'a> Deserialize<'a>,
    Err: Into<IpcError>,
>(
    f: usize,
    pipe: &mut BidirPipe,
) -> Result<()> {
    let f: fn(Args) -> Result<Res, Err> = unsafe { std::mem::transmute(f) };
    let args = pipe.read_serde()?;
    let res: IpcResult<Res> = f(args).map_err(|err| err.into());
    pipe.write_serde(&res)?;
    Ok(())
}

fn zygote_main(mut pipe: BidirPipe) -> anyhow::Result<()> {
    loop {
        let [f, runner]: [usize; 2] = pipe.read_serde()?;
        let runner: fn(usize, &mut BidirPipe) -> Result<()> =
            unsafe { std::mem::transmute(runner) };
        runner(f, &mut pipe)?;
    }
}

#[cfg(test)]
mod test {
    use std::sync::LazyLock;

    use anyhow::bail;

    use super::Zygote;

    static NAME: LazyLock<String> = LazyLock::new(|| String::from("Zygote"));

    fn say_hi_ok(name: String) -> anyhow::Result<String> {
        Ok(format!("hello {name}"))
    }

    fn say_hi_err(_: String) -> anyhow::Result<String> {
        bail!("that didn't work")
    }

    #[test]
    fn task_success() {
        let res = Zygote::global().run(say_hi_ok, &NAME).unwrap();
        assert_eq!(res, "hello Zygote");
    }

    #[test]
    fn task_failure() {
        let res = Zygote::global().run(say_hi_err, &NAME).unwrap_err();
        assert_eq!(res.to_string(), "that didn't work");
    }

    #[test]
    fn many_calls() {
        Zygote::global().run(say_hi_err, &NAME).unwrap_err();
        Zygote::global().run(say_hi_err, &NAME).unwrap_err();
        Zygote::global().run(say_hi_ok, &NAME).unwrap();
        Zygote::global().run(say_hi_ok, &NAME).unwrap();
        Zygote::global().run(say_hi_err, &NAME).unwrap_err();
        Zygote::global().run(say_hi_ok, &NAME).unwrap();
    }
}
