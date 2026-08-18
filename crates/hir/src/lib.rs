//! Semantic representation lowered exclusively from the canonical CST.

use rowan::ast::AstNode;
use serde::{Deserialize, Serialize};
use webtest_syntax::ast::{self, BrowserOperation};
use webtest_text::{FileId, SyntaxOrigin};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TestId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StepId(pub u32);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HirFile {
    pub tests: Vec<HirTest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirTest {
    pub id: TestId,
    pub name: String,
    pub body: Vec<HirStmt>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirStmt {
    Browser(HirBrowserBlock),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirBrowserBlock {
    pub operations: Vec<HirBrowserOp>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirBrowserOp {
    Open(HirOpen),
    Click(HirClick),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirOpen {
    pub url: String,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirClick {
    pub locator: HirLocator,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HirLocator {
    pub kind: HirLocatorKind,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HirLocatorKind {
    Id(String),
}

pub fn lower(file: FileId, parsed: &webtest_syntax::Parse) -> HirFile {
    let Some(root) = ast::Root::cast(parsed.syntax()) else {
        return HirFile::default();
    };

    let tests = root
        .tests()
        .enumerate()
        .filter_map(|(index, test)| lower_test(file, index as u32, test))
        .collect();
    HirFile { tests }
}

fn lower_test(file: FileId, id: u32, test: ast::TestDecl) -> Option<HirTest> {
    let name = test.name()?.value()?;
    let body = test
        .browser_blocks()
        .map(|block| HirStmt::Browser(lower_browser_block(file, block)))
        .collect();
    Some(HirTest {
        id: TestId(id),
        name,
        body,
        origin: SyntaxOrigin::new(file, test.syntax().text_range()),
    })
}

fn lower_browser_block(file: FileId, block: ast::BrowserBlock) -> HirBrowserBlock {
    let operations = block
        .operations()
        .filter_map(|operation| match operation {
            BrowserOperation::Open(statement) => {
                let url = statement.url()?.value()?;
                Some(HirBrowserOp::Open(HirOpen {
                    url,
                    origin: SyntaxOrigin::new(file, statement.syntax().text_range()),
                }))
            }
            BrowserOperation::Click(statement) => {
                let locator = match statement.locator()? {
                    ast::Locator::Id(locator) => HirLocator {
                        kind: HirLocatorKind::Id(locator.value()?.value()?),
                        origin: SyntaxOrigin::new(file, locator.syntax().text_range()),
                    },
                };
                Some(HirBrowserOp::Click(HirClick {
                    locator,
                    origin: SyntaxOrigin::new(file, statement.syntax().text_range()),
                }))
            }
        })
        .collect();

    HirBrowserBlock {
        operations,
        origin: SyntaxOrigin::new(file, block.syntax().text_range()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_origin_is_precise() {
        let source = "test \"x\" { browser { click id(\"foo\") } }";
        let hir = lower(FileId::new(7), &webtest_syntax::parse(source));
        let HirStmt::Browser(block) = &hir.tests[0].body[0];
        let HirBrowserOp::Click(click) = &block.operations[0] else {
            panic!("expected click");
        };
        let range = click.locator.origin.range;
        assert_eq!(
            &source[u32::from(range.start()) as usize..u32::from(range.end()) as usize],
            "id(\"foo\")"
        );
    }
}
