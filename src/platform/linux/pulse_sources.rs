use hbb_common::{anyhow::anyhow, bail, ResultType};
use pulse::{
    callbacks::ListResult,
    context::{
        introspect::{ServerInfo, SourceInfo},
        Context, FlagSet, State as ContextState,
    },
    mainloop::standard::{IterateResult, Mainloop},
    operation::{Operation, State as OperationState},
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Source = (String, String);

struct Connection {
    context: Context,
    mainloop: Mainloop,
}

impl Connection {
    fn open() -> ResultType<Self> {
        let mainloop =
            Mainloop::new().ok_or_else(|| anyhow!("could not create audio event loop"))?;
        let context = Context::new(&mainloop, "XSCC audio input")
            .ok_or_else(|| anyhow!("could not create audio context"))?;
        let mut connection = Self { context, mainloop };
        connection.context.connect(None, FlagSet::NOFLAGS, None)?;
        while connection.context.get_state() != ContextState::Ready {
            connection.iterate()?;
        }
        Ok(connection)
    }

    fn iterate(&mut self) -> ResultType<()> {
        match self.context.get_state() {
            ContextState::Failed | ContextState::Terminated => {
                bail!("audio connection ended: {}", self.context.errno())
            }
            _ => {}
        }
        match self.mainloop.iterate(true) {
            IterateResult::Success(_) => Ok(()),
            IterateResult::Quit(code) => bail!("audio event loop exited with {}", code.0),
            IterateResult::Err(error) => Err(error.into()),
        }
    }

    fn wait<T: ?Sized>(&mut self, mut operation: Operation<T>) -> ResultType<()> {
        while operation.get_state() == OperationState::Running {
            if let Err(error) = self.iterate() {
                operation.cancel();
                return Err(error);
            }
        }
        if operation.get_state() == OperationState::Cancelled {
            bail!("audio server cancelled its reply: {}", self.context.errno());
        }
        Ok(())
    }

    fn sources(&mut self, name: Option<&str>) -> ResultType<Vec<Source>> {
        let sources = Rc::new(RefCell::new(Vec::new()));
        let received_end = Rc::new(Cell::new(false));
        let output = Rc::clone(&sources);
        let complete = Rc::clone(&received_end);
        let receive = move |event: ListResult<&SourceInfo>| match event {
            ListResult::Item(source) => output.borrow_mut().push((
                source.name.as_deref().unwrap_or_default().to_owned(),
                source.description.as_deref().unwrap_or_default().to_owned(),
            )),
            ListResult::End => complete.set(true),
            ListResult::Error => {}
        };
        let introspector = self.context.introspect();
        let operation = match name {
            Some(name) => introspector.get_source_info_by_name(name, receive),
            None => introspector.get_source_info_list(receive),
        };
        self.wait(operation)?;
        if !received_end.get() {
            bail!("could not read audio inputs: {}", self.context.errno());
        }
        Ok(sources.take())
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

pub(super) fn list() -> ResultType<Vec<Source>> {
    Connection::open()?.sources(None)
}

fn default_input_name(server: &ServerInfo) -> Option<String> {
    server.default_source_name.as_deref().map(str::to_owned)
}

pub(super) fn default() -> ResultType<Option<Source>> {
    let mut connection = Connection::open()?;
    let default_name = Rc::new(Cell::new(None));
    let output = Rc::clone(&default_name);
    let operation = connection
        .context
        .introspect()
        .get_server_info(move |server| {
            output.set(Some(default_input_name(server)));
        });
    connection.wait(operation)?;
    match default_name.take() {
        Some(Some(name)) => Ok(connection.sources(Some(&name))?.into_iter().next()),
        Some(None) => Ok(None),
        None => bail!(
            "audio server information was unavailable: {}",
            connection.context.errno()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_input_uses_the_recording_source() {
        let mut server = ServerInfo {
            user_name: None,
            host_name: None,
            server_version: None,
            server_name: None,
            sample_spec: pulse::sample::Spec {
                format: pulse::sample::Format::S16le,
                rate: 48_000,
                channels: 2,
            },
            default_sink_name: Some("speaker".into()),
            default_source_name: Some("microphone".into()),
            cookie: 0,
            channel_map: Default::default(),
        };
        assert_eq!(default_input_name(&server).as_deref(), Some("microphone"));
        server.default_source_name = None;
        assert_eq!(default_input_name(&server), None);
    }
}
