//! Runtime-facing test plan. The runtime never inspects syntax or HIR.

use serde::{Deserialize, Serialize};
use webtest_hir::{HirBrowserOp, HirFile, HirLocatorKind, HirStmt, StepId, TestId};
use webtest_text::{FileId, SourceRevision, SyntaxOrigin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestPlan {
    pub file: FileId,
    pub source_revision: SourceRevision,
    pub tests: Vec<PlannedTest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedTest {
    pub id: TestId,
    pub name: String,
    pub steps: Vec<PlannedStep>,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedStep {
    pub id: StepId,
    pub operation: TestOperation,
    pub origin: SyntaxOrigin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestOperation {
    Browser(BrowserOperation),
    Assertion(AssertionOperation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrowserOperation {
    Open { url: String },
    Click { locator: Locator },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssertionOperation {
    Visible { locator: Locator },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Locator {
    Id(String),
    Text(String),
}

pub fn lower(file: FileId, source_revision: SourceRevision, hir: &HirFile) -> TestPlan {
    let mut next_step = 0u32;
    let tests = hir
        .tests
        .iter()
        .map(|test| {
            let mut steps = Vec::new();
            for statement in &test.body {
                let HirStmt::Browser(block) = statement;
                for operation in &block.operations {
                    let (operation, origin) = match operation {
                        HirBrowserOp::Open(open) => (
                            TestOperation::Browser(BrowserOperation::Open {
                                url: open.url.clone(),
                            }),
                            open.origin,
                        ),
                        HirBrowserOp::Click(click) => {
                            let locator = lower_locator(&click.locator.kind);
                            (
                                TestOperation::Browser(BrowserOperation::Click { locator }),
                                click.locator.origin,
                            )
                        }
                        HirBrowserOp::ExpectVisible(expectation) => (
                            TestOperation::Assertion(AssertionOperation::Visible {
                                locator: lower_locator(&expectation.locator.kind),
                            }),
                            expectation.locator.origin,
                        ),
                    };
                    steps.push(PlannedStep {
                        id: StepId(next_step),
                        operation,
                        origin,
                    });
                    next_step += 1;
                }
            }
            PlannedTest {
                id: test.id,
                name: test.name.clone(),
                steps,
                origin: test.origin,
            }
        })
        .collect();
    TestPlan {
        file,
        source_revision,
        tests,
    }
}

fn lower_locator(locator: &HirLocatorKind) -> Locator {
    match locator {
        HirLocatorKind::Id(value) => Locator::Id(value.clone()),
        HirLocatorKind::Text(value) => Locator::Text(value.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_has_deterministic_steps_revision_and_locator_range() {
        let source = "test \"x\" { browser { open \"about:blank\" click id(\"foo\") expect text(\"done\").visible } }";
        let file = FileId::new(4);
        let revision = SourceRevision::of(source);
        let parsed = webtest_syntax::parse(source);
        let hir = webtest_hir::lower(file, &parsed);
        let plan = lower(file, revision, &hir);
        assert_eq!(plan.source_revision, revision);
        assert_eq!(plan.tests[0].steps[0].id, StepId(0));
        assert_eq!(plan.tests[0].steps[1].id, StepId(1));
        assert_eq!(plan.tests[0].steps[2].id, StepId(2));
        let range = plan.tests[0].steps[1].origin.range;
        assert_eq!(
            &source[u32::from(range.start()) as usize..u32::from(range.end()) as usize],
            "id(\"foo\")"
        );
        assert_eq!(
            plan.tests[0].steps[2].operation,
            TestOperation::Assertion(AssertionOperation::Visible {
                locator: Locator::Text("done".into())
            })
        );
    }
}
