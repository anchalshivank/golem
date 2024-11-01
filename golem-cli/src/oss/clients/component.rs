// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::io::Read;
use std::path::PathBuf;
use async_trait::async_trait;
use golem_wasm_rpc_stubgen::model::oam::{Application};
use crate::clients::component::ComponentClient;
use golem_common::uri::oss::urn::ComponentUrn;
use tokio::fs::File;
use tracing::info;
use golem_client::model::ComponentType;
use crate::model::component::Component;
use crate::model::{ComponentName, GolemError, PathBufOrStdin};
use crate::oss::model::OssContext;

#[derive(Debug, Clone)]
pub struct ComponentClientLive<C: golem_client::api::ComponentClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ComponentClient + Sync + Send> ComponentClient
    for ComponentClientLive<C>
{
    type ProjectContext = OssContext;

    async fn get_metadata(
        &self,
        component_urn: &ComponentUrn,
        version: u64,
    ) -> Result<Component, GolemError> {
        info!("Getting component version");

        Ok(self
            .client
            .get_component_metadata(&component_urn.id.0, &version.to_string())
            .await?
            .into())
    }

    async fn get_latest_metadata(
        &self,
        component_urn: &ComponentUrn,
    ) -> Result<Component, GolemError> {
        info!("Getting latest component version");

        Ok(self
            .client
            .get_latest_component_metadata(&component_urn.id.0)
            .await?
            .into())
    }

    async fn find(
        &self,
        name: Option<ComponentName>,
        _project: &Option<Self::ProjectContext>,
    ) -> Result<Vec<Component>, GolemError> {
        info!("Getting components");

        let name = name.map(|n| n.0);

        let components = self.client.get_components(name.as_deref()).await?;
        Ok(components.into_iter().map(|c| c.into()).collect())
    }

    async fn add(
        &self,
        name: ComponentName,
        file: PathBufOrStdin,
        project: &Option<Self::ProjectContext>,
        component_type: ComponentType,
        ifs: PathBuf,
    ) -> Result<Component, GolemError> {
        info!("Adding component {name:?} from {file:?}");

        let component = match file {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path.clone())
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;
                let ifs_file = File::open(ifs.clone())
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;
                self.client
                    .create_component(&name.0, Some(&component_type), file, Some(ifs_file))
                    .await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes) // TODO: steaming request from stdin
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                self.client
                    .create_component(&name.0, Some(&component_type), bytes.clone(), Some(bytes))
                    .await?
            }
        };

        Ok(component.into())
    }

    async fn update(
        &self,
        urn: ComponentUrn,
        file: PathBufOrStdin,
        component_type: Option<ComponentType>,
        ifs: PathBuf,
    ) -> Result<Component, GolemError> {
        info!("Updating component {urn} from {file:?}");

        let component = match file {
            PathBufOrStdin::Path(path) => {
                let file = File::open(path)
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;
                let ifs_file = File::open(ifs.clone())
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;

                self.client
                    .update_component(&urn.id.0, component_type.as_ref(), Some(file), Some(ifs_file))
                    .await?
            }
            PathBufOrStdin::Stdin => {
                let mut bytes = Vec::new();

                let _ = std::io::stdin()
                    .read_to_end(&mut bytes)
                    .map_err(|e| GolemError(format!("Failed to read stdin: {e:?}")))?;

                let ifs_file = File::open(ifs.clone())
                    .await
                    .map_err(|e| GolemError(format!("Can't open component file: {e}")))?;
                self.client
                    .update_component(&urn.id.0, component_type.as_ref(), Some(bytes), Some(ifs_file))
                    .await?
            }
        };

        Ok(component.into())
    }
}
