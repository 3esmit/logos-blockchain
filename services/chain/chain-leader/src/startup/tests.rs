use std::time::Duration;

use async_trait::async_trait;
use overwatch::{
    DynError, OpaqueServiceResourcesHandle, derive_services,
    overwatch::{Overwatch, OverwatchRunner},
    services::{
        ServiceCore, ServiceData,
        state::{NoOperator, NoState},
    },
};
use tokio::sync::watch;

use super::wait_for_dependencies;

struct Gate<const ID: usize> {
    resources: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
}

impl<const ID: usize> ServiceData for Gate<ID> {
    type Settings = watch::Receiver<bool>;
    type State = NoState<Self::Settings>;
    type StateOperator = NoOperator<Self::State>;
    type Message = ();
}

#[async_trait]
impl<const ID: usize> ServiceCore<RuntimeServiceId> for Gate<ID> {
    fn init(
        resources: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
        _state: Self::State,
    ) -> Result<Self, DynError> {
        Ok(Self { resources })
    }

    async fn run(self) -> Result<(), DynError> {
        let mut ready = self
            .resources
            .settings_handle
            .notifier()
            .get_updated_settings();
        drop(ready.wait_for(|value| *value).await?);
        self.resources.status_updater.notify_ready();
        std::future::pending().await
    }
}

type Outcome = Result<(), String>;

struct Dependent {
    resources: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
}

impl ServiceData for Dependent {
    type Settings = watch::Sender<Option<Outcome>>;
    type State = NoState<Self::Settings>;
    type StateOperator = NoOperator<Self::State>;
    type Message = ();
}

#[async_trait]
impl ServiceCore<RuntimeServiceId> for Dependent {
    fn init(
        resources: OpaqueServiceResourcesHandle<Self, RuntimeServiceId>,
        _state: Self::State,
    ) -> Result<Self, DynError> {
        Ok(Self { resources })
    }

    async fn run(self) -> Result<(), DynError> {
        let result = wait_for_dependencies::<
            Gate<0>,
            Gate<0>,
            Gate<1>,
            Gate<0>,
            Gate<0>,
            Gate<0>,
            Gate<0>,
            _,
        >(&self.resources.overwatch_handle)
        .await
        .map_err(|error| error.to_string());
        self.resources
            .settings_handle
            .notifier()
            .get_updated_settings()
            .send(Some(result.clone()))
            .expect("Outcome receiver remains alive");
        result.map_err(std::io::Error::other)?;
        self.resources.status_updater.notify_ready();
        std::future::pending().await
    }
}

type Ordinary = Gate<0>;
type Recovering = Gate<1>;

#[derive_services]
struct Services {
    ordinary: Ordinary,
    recovering: Recovering,
    dependent: Dependent,
}

struct Fixture {
    app: Overwatch<RuntimeServiceId>,
    _ordinary: watch::Sender<bool>,
    recovering: watch::Sender<bool>,
    outcome: watch::Receiver<Option<Outcome>>,
}

impl Fixture {
    async fn start(ordinary_ready: bool, recovery_ready: bool) -> Self {
        let (ordinary, ordinary_settings) = watch::channel(ordinary_ready);
        let (recovering, recovering_settings) = watch::channel(recovery_ready);
        let (dependent, outcome) = watch::channel(None);
        let app = OverwatchRunner::<Services>::run(
            ServicesServiceSettings {
                ordinary: ordinary_settings,
                recovering: recovering_settings,
                dependent,
            },
            Some(tokio::runtime::Handle::current()),
        )
        .expect("Test runtime starts");
        app.handle()
            .start_all_services()
            .await
            .expect("Test services start");
        Self {
            app,
            _ordinary: ordinary,
            recovering,
            outcome,
        }
    }

    async fn outcome(&mut self) -> Outcome {
        tokio::time::timeout(
            Duration::from_secs(65),
            self.outcome.wait_for(Option::is_some),
        )
        .await
        .expect("Dependency wait reports within the controlled clock budget")
        .expect("Outcome sender remains alive")
        .as_ref()
        .expect("Outcome was observed")
        .clone()
    }

    async fn shutdown(self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            self.app
                .handle()
                .shutdown()
                .await
                .expect("Shutdown acknowledged");
            self.app.wait_finished().await;
        })
        .await
        .expect("Shutdown cancels pending readiness waits");
    }
}

#[tokio::test(start_paused = true)]
async fn recovery_can_outlast_the_ordinary_startup_deadline() {
    let mut fixture = Fixture::start(true, false).await;
    // Paused Tokio time advances only after the startup tasks have blocked.
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert!(fixture.outcome.borrow().is_none());
    fixture
        .recovering
        .send(true)
        .expect("Recovery service remains alive");
    assert_eq!(fixture.outcome().await, Ok(()));
    fixture.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn ordinary_dependencies_still_time_out() {
    let mut fixture = Fixture::start(false, true).await;
    let error = fixture
        .outcome()
        .await
        .expect_err("Ordinary readiness must remain bounded");
    assert!(error.contains("Ordinary"), "{error}");
    fixture.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_pending_recovery() {
    let fixture = Fixture::start(true, false).await;
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert!(fixture.outcome.borrow().is_none());
    fixture.shutdown().await;
}
