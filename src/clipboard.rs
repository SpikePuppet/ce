pub trait ClipboardProvider {
    type Error;

    fn read_text(&mut self) -> Result<String, Self::Error>;
    fn write_text(&mut self, text: String) -> Result<(), Self::Error>;
}

#[derive(Default)]
pub struct SystemClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl SystemClipboard {
    fn get(&mut self) -> Result<&mut arboard::Clipboard, arboard::Error> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new()?);
        }
        Ok(self
            .clipboard
            .as_mut()
            .expect("clipboard was initialized above"))
    }
}

impl ClipboardProvider for SystemClipboard {
    type Error = arboard::Error;

    fn read_text(&mut self) -> Result<String, Self::Error> {
        self.get()?.get_text()
    }

    fn write_text(&mut self, text: String) -> Result<(), Self::Error> {
        self.get()?.set_text(text)
    }
}
