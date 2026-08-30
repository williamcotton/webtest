use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use webtest_browser::{
    Action, BrowserContextOptions, BrowserError, BrowserHost, EvidenceRequest, InspectionOptions,
    Locator, LocatorCandidateKind, LocatorState, SupportedAction,
};

use crate::ChromeHost;

#[tokio::test]
async fn real_chrome_clicks_and_checks_visible_text_when_available() {
    let host = ChromeHost::default();
    if host.locate().is_none() {
        return;
    }
    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        // Some build sandboxes prohibit loopback listeners. The same path is
        // exercised when the browser integration test runs outside them.
        return;
    };
    let address = listener.local_addr().expect("fixture address");
    let fixture = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await;
                let body = "<!doctype html><html><body><button id=\"submit\" onclick=\"const result=document.createElement('div');result.textContent='submitted';document.body.append(result)\">Submit</button><div style=\"display:none\">hidden</div></body></html>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("serve fixture");
            });
        }
    });
    let mut browser = host.start().await.expect("start Chrome");
    let mut page = browser.new_page().await.expect("create page");
    page.open(&format!("http://{address}"))
        .await
        .expect("navigate");
    page.click(&Locator::Id("submit".into()))
        .await
        .expect("click existing");
    page.expect_visible(&Locator::Text("submitted".into()))
        .await
        .expect("submitted text is visible");
    let hidden = page.expect_visible(&Locator::Text("hidden".into())).await;
    assert!(matches!(
        hidden,
        Err(BrowserError::LocatorNotVisible { .. })
    ));
    let missing = page.click(&Locator::Id("missing".into())).await;
    assert!(matches!(missing, Err(BrowserError::LocatorNotFound { .. })));
    drop(page);
    browser.close().await.expect("close and reap Chrome");
    fixture.abort();
}

#[tokio::test]
async fn real_chrome_runs_semantic_form_flow_with_physical_input_when_available() {
    let host = ChromeHost::default();
    if host.locate().is_none() {
        return;
    }
    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        return;
    };
    let address = listener.local_addr().expect("fixture address");
    let fixture = r#"<!doctype html><html><head><meta charset="utf-8"><title>Sign in</title></head><body>
            <form id="signin">
              <label>Email <input type="email" value="old@example.com"></label>
              <label>Password <input type="password"></label>
              <label>Biography <textarea></textarea></label>
              <label>Search <input type="search" placeholder="Search products"></label>
              <label>Timezone <select><option value="America/Chicago">America/Chicago</option></select></label>
              <label>Email notifications <input type="checkbox"></label>
              <label>SMS notifications <input type="checkbox" checked></label>
              <button type="button">Account</button>
              <button type="submit">Sign in</button>
              <button type="button" disabled>Unavailable</button>
              <label>City 🏙 <input type="text" placeholder="Montréal"></label>
              <button type="button" style="display:none">Hidden action</button>
            </form>
            <script>
              document.querySelector('button[type=submit]').addEventListener('click', event => {
                event.preventDefault();
                const values = Array.from(document.getElementById('signin').elements);
                history.pushState({}, '', '/dashboard');
                const result = document.createElement('div');
                const email = document.querySelector('input[type=email]').value;
                const password = document.querySelector('input[type=password]').value;
                result.textContent = email === 'alice@example.com' && password === 'secret'
                  ? 'Welcome, Alice' : `invalid:${email}:${password}`;
                document.body.append(result);
              });
              document.querySelector('input[type=search]').addEventListener('keydown', event => {
                if (event.key === 'Enter') {
                  const result = document.createElement('div'); result.textContent = 'Key pressed'; document.body.append(result);
                }
              });
            </script>
        </body></html>"#;
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept form request");
        let mut request = [0u8; 2048];
        let _ = stream.read(&mut request).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{fixture}",
            fixture.len()
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("serve form");
    });

    let mut browser = host.start().await.expect("start Chrome");
    let mut context = browser
        .new_context(&BrowserContextOptions::default())
        .await
        .expect("context");
    let mut page = context.new_page().await.expect("page");
    page.open(&format!("http://{address}/login"))
        .await
        .expect("open form");
    let inspection = page
        .inspect(&InspectionOptions::default())
        .await
        .expect("inspect form");
    let email = inspection
        .elements
        .iter()
        .find(|element| element.label.as_deref() == Some("Email"))
        .unwrap_or_else(|| panic!("email inspection: {inspection:#?}"));
    assert_eq!(email.preferred_locator.source, "label(\"Email\")");
    assert_eq!(
        email.supported_actions,
        vec![
            SupportedAction::Fill,
            SupportedAction::Type,
            SupportedAction::Press
        ]
    );
    let sign_in = inspection
        .elements
        .iter()
        .find(|element| element.accessible_name.as_deref() == Some("Sign in"))
        .expect("sign-in inspection");
    assert_eq!(
        sign_in.preferred_locator.source,
        "role(\"button\", name: \"Sign in\")"
    );
    let repair_hints = webtest_browser::locator_repair_hints(
        &Locator::Role {
            role: "button".into(),
            name: Some("Log in".into()),
        },
        &inspection,
        webtest_browser::MAX_CANDIDATES,
    );
    assert_eq!(
        repair_hints[0].replacement,
        webtest_browser::RepairReplacement::locator("role(\"button\", name: \"Sign in\")")
    );
    let unavailable = inspection
        .elements
        .iter()
        .find(|element| element.accessible_name.as_deref() == Some("Unavailable"))
        .expect("disabled inspection");
    assert_eq!(unavailable.states.enabled, Some(false));
    assert!(
        !unavailable
            .supported_actions
            .contains(&SupportedAction::Click)
    );
    assert!(
        inspection
            .elements
            .iter()
            .all(|element| { element.accessible_name.as_deref() != Some("Hidden action") })
    );
    assert!(inspection.elements.iter().all(|element| {
        std::iter::once(&element.preferred_locator)
            .chain(&element.alternate_locators)
            .all(|candidate| {
                !matches!(candidate.kind, LocatorCandidateKind::Text)
                    || !candidate.source.contains("secret")
            })
    }));
    assert!(
        inspection
            .elements
            .iter()
            .flat_map(|element| {
                std::iter::once(&element.preferred_locator).chain(&element.alternate_locators)
            })
            .all(|candidate| {
                !candidate.source.starts_with("css(") && !candidate.source.starts_with("xpath(")
            })
    );
    assert!(
        inspection
            .elements
            .iter()
            .any(|element| element.label.as_deref() == Some("City 🏙"))
    );
    page.perform(
        &Action::Fill {
            locator: Locator::Label("Email".into()),
            value: "alice@example.com".into(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("fill email");
    page.perform(
        &Action::Fill {
            locator: Locator::Label("Password".into()),
            value: "secret".into(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("fill password");
    page.perform(
        &Action::Type {
            locator: Locator::Label("Biography".into()),
            value: "hello".into(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("type biography");
    page.perform(
        &Action::Press {
            locator: Locator::Placeholder("Search products".into()),
            key: "Enter".into(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("press Enter");
    page.wait_for_locator(
        &Locator::Text("Key pressed".into()),
        LocatorState::Visible,
        Duration::from_secs(2),
    )
    .await
    .expect("key event was dispatched");
    page.perform(
        &Action::Select {
            locator: Locator::Label("Timezone".into()),
            option: "America/Chicago".into(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("select timezone");
    page.perform(
        &Action::Check {
            locator: Locator::Label("Email notifications".into()),
            checked: true,
        },
        Duration::from_secs(2),
    )
    .await
    .expect("check notifications");
    page.perform(
        &Action::Check {
            locator: Locator::Label("SMS notifications".into()),
            checked: false,
        },
        Duration::from_secs(2),
    )
    .await
    .expect("uncheck notifications");
    page.perform(
        &Action::Hover {
            locator: Locator::Text("Account".into()),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("hover account");
    page.perform(
        &Action::Click {
            locator: Locator::Role {
                role: "button".into(),
                name: Some("Sign in".into()),
            },
        },
        Duration::from_secs(2),
    )
    .await
    .expect("physical sign-in click");
    if let Err(error) = page
        .wait_for_locator(
            &Locator::Text("Welcome, Alice".into()),
            LocatorState::Visible,
            Duration::from_secs(2),
        )
        .await
    {
        let evidence = page
            .capture_evidence(&EvidenceRequest {
                locator: None,
                include_screenshot: false,
                include_dom: true,
                max_dom_bytes: 4096,
                redactions: vec!["secret".into()],
                redacted_query_parameters: Vec::new(),
            })
            .await;
        panic!(
            "welcome assertion: {error}; DOM: {:?}; console: {:?}",
            evidence.dom_snapshot, evidence.console_errors
        );
    }
    page.wait_for_url(
        &format!("http://{address}/dashboard"),
        Duration::from_secs(2),
    )
    .await
    .expect("dashboard URL");
    page.wait_for_locator(
        &Locator::Label("Email notifications".into()),
        LocatorState::Checked,
        Duration::from_secs(2),
    )
    .await
    .expect("checked assertion");
    page.wait_for_locator(
        &Locator::Label("SMS notifications".into()),
        LocatorState::Unchecked,
        Duration::from_secs(2),
    )
    .await
    .expect("unchecked assertion");
    let evidence = page
        .capture_evidence(&EvidenceRequest {
            locator: Some(Locator::Role {
                role: "button".into(),
                name: Some("Sign in".into()),
            }),
            include_screenshot: true,
            include_dom: true,
            max_dom_bytes: 512,
            redactions: vec!["secret".into()],
            redacted_query_parameters: Vec::new(),
        })
        .await;
    assert!(
        evidence
            .screenshot_png
            .as_deref()
            .is_some_and(|png| png.starts_with(&[137, 80, 78, 71]))
    );
    assert!(
        evidence
            .dom_snapshot
            .as_ref()
            .is_some_and(|dom| dom.len() <= 512)
    );
    drop(page);
    context.close().await.expect("close context");
    browser.close().await.expect("close browser");
}

#[tokio::test]
async fn actionability_failures_are_distinct_and_candidate_evidence_is_bounded() {
    let host = ChromeHost::default();
    if host.locate().is_none() {
        return;
    }

    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else {
        return;
    };

    let address = listener.local_addr().expect("fixture address");

    let body = r#"<!doctype html><style>
            #covered { position:absolute;left:20px;top:20px;width:100px;height:30px }
            #overlay { position:absolute;left:20px;top:20px;width:100px;height:30px;z-index:2 }
            #unstable { position:absolute;top:80px;animation:move .08s infinite alternate linear }
            @keyframes move { from { left:10px } to { left:200px } }
        </style><body>
            <button id="disabled" disabled>disabled</button>
            <button id="covered">covered</button><div id="overlay">overlay</div>
            <button id="unstable">unstable</button>
    
            <button class="duplicate">same</button><button class="duplicate">same</button>
            <button class="duplicate">same</button><button class="duplicate">same</button>
            <button class="duplicate">same</button><button class="duplicate">same</button>
    
            <button style="display:none" id="hidden">hidden</button>
    
            <input placeholder="Search products" data-testid="search-box">
    
            <div id="space"> Hello
                 World </div>
    
            <div id="shadow-host"></div>
            <iframe srcdoc="<button>Inside frame</button>"></iframe>
    
            <script>
                document
                    .getElementById('shadow-host')
                    .attachShadow({mode:'open'})
                    .innerHTML='<button>Inside shadow</button>';
            </script>
    
            <div id="transient-host"></div>
    
            <script>
                window.startTransient = () => {
                    const host = document.getElementById('transient-host');
    
                    // Start with no matches.
                    host.innerHTML = '';
    
                    // Then become ambiguous for long enough that the actionability
                    // loop should reliably observe the second failure state.
                    setTimeout(() => {
                        host.innerHTML =
                            '<button class="transient">one</button>' +
                            '<button class="transient">two</button>';
                    }, 80);
    
                    // Return to no matches. The action never becomes actionable,
                    // but multiple distinct failure reasons are observed.
                    setTimeout(() => {
                        host.innerHTML = '';
                    }, 180);
                };
            </script>
        </body>"#;

    tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .expect("accept actionability request");

        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await;

        let response = format!(
            "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
            body.len()
        );

        stream
            .write_all(response.as_bytes())
            .await
            .expect("serve actionability fixture");
    });

    let mut browser = host.start().await.expect("start Chrome");
    let mut page = browser.new_page().await.expect("page");

    page.open(&format!("http://{address}"))
        .await
        .expect("open fixture");

    let click = |locator| Action::Click { locator };

    assert!(matches!(
        page.perform(
            &click(Locator::Id("missing".into())),
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::LocatorNotFound { .. })
    ));

    assert!(matches!(
        page.perform(
            &click(Locator::Css(".duplicate".into())),
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::LocatorAmbiguous { .. })
    ));

    assert!(matches!(
        page.perform(
            &click(Locator::Id("disabled".into())),
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::ElementDisabled { .. })
    ));

    assert!(matches!(
        page.perform(
            &click(Locator::Id("covered".into())),
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::ElementObscured { .. })
    ));

    assert!(matches!(
        page.perform(
            &click(Locator::Id("hidden".into())),
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::LocatorNotVisible { .. })
    ));

    assert!(matches!(
        page.perform(
            &click(Locator::Id("unstable".into())),
            Duration::from_millis(140)
        )
        .await,
        Err(BrowserError::ElementUnstable { .. })
    ));

    assert!(matches!(
        page.perform(&click(Locator::Css("[".into())), Duration::from_millis(100))
            .await,
        Err(BrowserError::LocatorInvalid { .. })
    ));

    page.evaluate("window.startTransient()")
        .await
        .expect("start transient fixture");

    assert!(matches!(
        page.perform(
            &click(Locator::Css(".transient".into())),
            Duration::from_millis(300)
        )
        .await,
        Err(BrowserError::ActionTimeout { .. })
    ));

    page.wait_for_locator(
        &Locator::Placeholder("Search products".into()),
        LocatorState::Visible,
        Duration::from_secs(1),
    )
    .await
    .expect("placeholder locator");

    page.wait_for_locator(
        &Locator::TestId("search-box".into()),
        LocatorState::Visible,
        Duration::from_secs(1),
    )
    .await
    .expect("test-ID locator");

    page.wait_for_locator(
        &Locator::Text("Hello World".into()),
        LocatorState::Visible,
        Duration::from_secs(1),
    )
    .await
    .expect("rendered whitespace normalization");

    page.wait_for_locator(
        &Locator::XPath("//*[@id='space']".into()),
        LocatorState::Visible,
        Duration::from_secs(1),
    )
    .await
    .expect("XPath locator");

    assert!(matches!(
        page.wait_for_locator(
            &Locator::Text("Inside shadow".into()),
            LocatorState::Visible,
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::LocatorNotFound { .. })
    ));

    assert!(matches!(
        page.wait_for_locator(
            &Locator::Text("Inside frame".into()),
            LocatorState::Visible,
            Duration::from_millis(100)
        )
        .await,
        Err(BrowserError::LocatorNotFound { .. })
    ));

    let evidence = page
        .capture_evidence(&EvidenceRequest {
            locator: Some(Locator::Css(".duplicate".into())),
            include_screenshot: false,
            include_dom: false,
            max_dom_bytes: 0,
            redactions: Vec::new(),
            redacted_query_parameters: Vec::new(),
        })
        .await;

    assert_eq!(evidence.candidates.len(), 5);

    browser.close().await.expect("close browser");
}
