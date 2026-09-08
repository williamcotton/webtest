//! Composition of worker-owned application lifecycles and matching runtime services.
use std::net::TcpListener;

use webtest_project::Project;
use webtest_runtime::TestWorker;

use crate::{
    error::AppError,
    provider_composition::{RuntimeApplication, runtime_application, runtime_provider_registry},
    runtime_configuration::runner_options,
};

pub(crate) struct WorkerApplications {
    pub workers: Vec<TestWorker>,
    applications: Vec<(Project, RuntimeApplication, Option<TcpListener>)>,
}

impl WorkerApplications {
    pub fn prepare(project: &Project, count: usize) -> Result<Self, AppError> {
        if project.config.server.app.as_ref().is_some_and(|app| {
            app.adapter == webtest_project::ServerAppAdapter::Command
                || (app.adapter == webtest_project::ServerAppAdapter::Bridge
                    && app.transport == webtest_project::ServerAppTransport::Stdio)
        }) {
            return Err(AppError::usage(
                "worker-owned applications require a socket bridge or HTTP adapter; command and stdio adapters must use --jobs 1",
            ));
        }
        let mut workers = Vec::new();
        let mut applications = Vec::new();
        for worker in 0..count {
            // Reserve distinct ports while preparing the pool. Each reservation
            // is released immediately before its application is spawned.
            let reservation = TcpListener::bind("127.0.0.1:0").map_err(AppError::infrastructure)?;
            let port = reservation
                .local_addr()
                .map_err(AppError::infrastructure)?
                .port();
            let project = worker_project(project, worker, port)?;
            let options = runner_options(&project);
            let providers = runtime_provider_registry(&project, &options)?;
            let application = runtime_application(&project, providers.app)
                .ok_or_else(|| AppError::internal("worker application lifecycle is missing"))?;
            workers.push(TestWorker {
                options,
                providers: providers.registry,
            });
            applications.push((project, application, Some(reservation)));
        }
        Ok(Self {
            workers,
            applications,
        })
    }

    pub async fn start(&mut self) -> Result<(), webtest_provider::ProviderError> {
        for (project, application, reservation) in &mut self.applications {
            drop(reservation.take());
            application.start(project).await?;
        }
        Ok(())
    }

    /// Always called, including after partial startup. Every attempted lifecycle
    /// is awaited and every shutdown failure remains available to the reporter.
    pub async fn shutdown(&self) -> Vec<webtest_provider::ProviderError> {
        let mut failures = Vec::new();
        for (_, application, _) in &self.applications {
            if let Err(error) = application.shutdown().await {
                failures.push(error);
            }
        }
        failures
    }
}

fn worker_project(project: &Project, worker: usize, port: u16) -> Result<Project, AppError> {
    let mut project = project.clone();
    let original = project
        .config
        .browser
        .base_url
        .as_deref()
        .or(project.config.server.base_url.as_deref())
        .or(project
            .config
            .server
            .app
            .as_ref()
            .and_then(|app| app.http_base_url.as_deref()))
        .or(project
            .config
            .app
            .as_ref()
            .and_then(|app| app.health.as_ref())
            .map(|health| health.url.as_str()))
        .unwrap_or("http://127.0.0.1");
    let origin = url::Url::parse(original).map_err(AppError::usage)?;
    if !loopback(&origin) {
        return Err(AppError::usage(
            "worker-owned applications require a loopback application base URL",
        ));
    }
    let mut endpoint = origin.clone();
    endpoint
        .set_host(Some("127.0.0.1"))
        .map_err(AppError::usage)?;
    endpoint
        .set_port(Some(port))
        .map_err(|()| AppError::usage("application URL cannot accept a worker port"))?;
    endpoint.set_path("");
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let rebase = |value: &mut String| -> Result<(), AppError> {
        let mut url = url::Url::parse(value).map_err(AppError::usage)?;
        if loopback(&url)
            && url.scheme() == origin.scheme()
            && url.port_or_known_default() == origin.port_or_known_default()
        {
            url.set_host(Some("127.0.0.1")).map_err(AppError::usage)?;
            url.set_port(Some(port))
                .map_err(|()| AppError::usage("application URL cannot accept a worker port"))?;
            *value = url.into();
        }
        Ok(())
    };
    for value in [
        &mut project.config.browser.base_url,
        &mut project.config.server.base_url,
    ]
    .into_iter()
    .flatten()
    {
        rebase(value)?;
    }
    if let Some(value) = project
        .config
        .server
        .app
        .as_mut()
        .and_then(|app| app.http_base_url.as_mut())
    {
        rebase(value)?;
    }
    let app = project
        .config
        .app
        .as_mut()
        .ok_or_else(|| AppError::usage("worker applications require [app]"))?;
    if !app.owned {
        return Err(AppError::usage(
            "worker applications require app.owned = true",
        ));
    }
    if let Some(health) = &mut app.health {
        rebase(&mut health.url)?;
    }
    app.environment
        .insert("WEBTEST_WORKER_ID".into(), worker.to_string());
    app.environment
        .insert("WEBTEST_APP_PORT".into(), port.to_string());
    app.environment.insert(
        "WEBTEST_APP_URL".into(),
        endpoint.as_str().trim_end_matches('/').into(),
    );
    Ok(project)
}

fn loopback(url: &url::Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_routes_and_environment_share_one_endpoint_and_preserve_paths() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("webtest.toml"),
            r#"
            [app]
            command = "application"
            [app.environment]
            EXISTING = "preserved"
            [app.health]
            url = "http://localhost:5055/healthz"
            [browser]
            base_url = "http://127.0.0.1:5055/ui/"
            [server]
            base_url = "http://localhost:5055/api/"
        "#,
        )
        .unwrap();
        let project = webtest_project::discover(&[directory.path().to_owned()]).unwrap();
        let worker = worker_project(&project, 2, 54321).unwrap();
        assert_eq!(
            worker.config.browser.base_url.as_deref(),
            Some("http://127.0.0.1:54321/ui/")
        );
        assert_eq!(
            worker.config.server.base_url.as_deref(),
            Some("http://127.0.0.1:54321/api/")
        );
        let app = worker.config.app.unwrap();
        assert_eq!(app.health.unwrap().url, "http://127.0.0.1:54321/healthz");
        assert_eq!(app.environment["WEBTEST_WORKER_ID"], "2");
        assert_eq!(app.environment["WEBTEST_APP_PORT"], "54321");
        assert_eq!(app.environment["WEBTEST_APP_URL"], "http://127.0.0.1:54321");
        assert_eq!(app.environment["EXISTING"], "preserved");
        assert_eq!(
            project.config.browser.base_url.as_deref(),
            Some("http://127.0.0.1:5055/ui/")
        );
    }
}
