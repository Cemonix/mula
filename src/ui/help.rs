use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Text, ToSpan},
    widgets::{Block, Clear, Padding, Widget},
};

use crate::keys::Binding;

pub struct Help<'b, T> {
    bindings: &'b [Binding<T>],
}

impl<'b, T> Help<'b, T> {
    const WIDTH: u16 = 80;
    const HEIGHT: u16 = 30;

    pub fn new(bindings: &'b [Binding<T>]) -> Self {
        Self { bindings }
    }
}

impl<'b, T> Widget for Help<'b, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let [area] = Layout::horizontal([Constraint::Length(Self::WIDTH)])
            .flex(Flex::Center)
            .areas(area);
        let [area] = Layout::vertical([Constraint::Length(Self::HEIGHT)])
            .flex(Flex::Center)
            .areas(area);

        Clear.render(area, buf);

        let block = Block::bordered()
            .title_top(Line::from("Help").centered())
            .padding(Padding::horizontal(1))
            .style(Style::new().bg(Color::Black).fg(Color::Blue));
        let inner = block.inner(area);
        block.render(area, buf);

        let max_key_len = self
            .bindings
            .iter()
            .map(|b| b.key.to_span().width() as u16)
            .max()
            .unwrap_or(0);

        let columns = Layout::horizontal([Constraint::Length(max_key_len + 2), Constraint::Min(0)])
            .split(inner);

        Text::from_iter(
            self.bindings
                .iter()
                .map(|b| Line::from(b.key.to_span().bold().white())),
        )
        .render(columns[0], buf);

        Text::from_iter(
            self.bindings
                .iter()
                .map(|b| Line::from(b.help.to_span().white())),
        )
        .render(columns[1], buf);
    }
}

#[cfg(test)]
mod help_tests {
    use super::*;
    use crate::keys::BROWSE_KEYS;

    #[test]
    fn browse_keys_fit_the_help_overlay() {
        let inner_height = Help::<()>::HEIGHT - 2; // top and bottom border
        assert!(BROWSE_KEYS.len() <= inner_height as usize);
    }
}
