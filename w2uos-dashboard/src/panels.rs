use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PanelType {
    Table,
    Chart,
    Log,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PanelDataSource {
    Rest { path: String },
    WsSubject { subject: String },
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardPanelConfig {
    pub id: String,
    pub title: String,
    pub panel_type: PanelType,
    pub data_source: PanelDataSource,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DashboardLayout {
    pub panels: Vec<DashboardPanelConfig>,
}

impl DashboardLayout {
    pub fn default() -> Self {
        Self {
            panels: vec![
                DashboardPanelConfig {
                    id: "node_status".into(),
                    title: "Node Status".into(),
                    panel_type: PanelType::Table,
                    data_source: PanelDataSource::Rest {
                        path: "/status".into(),
                    },
                },
                DashboardPanelConfig {
                    id: "markets".into(),
                    title: "Market Watch".into(),
                    panel_type: PanelType::Table,
                    data_source: PanelDataSource::WsSubject {
                        subject: "market.*".into(),
                    },
                },
                DashboardPanelConfig {
                    id: "positions".into(),
                    title: "Positions".into(),
                    panel_type: PanelType::Table,
                    data_source: PanelDataSource::Rest {
                        path: "/positions".into(),
                    },
                },
                DashboardPanelConfig {
                    id: "performance".into(),
                    title: "Performance".into(),
                    panel_type: PanelType::Chart,
                    data_source: PanelDataSource::Rest {
                        path: "/metrics/latency/summary".into(),
                    },
                },
                DashboardPanelConfig {
                    id: "logs".into(),
                    title: "Log Console".into(),
                    panel_type: PanelType::Log,
                    data_source: PanelDataSource::WsSubject {
                        subject: "log.event".into(),
                    },
                },
            ],
        }
    }
}
