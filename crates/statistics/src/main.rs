use anyhow::Result;
use chrono::serde::ts_seconds;
use chrono::{DateTime, TimeZone, Utc};
use plotters::prelude::*;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct Db {
    pub entries: Vec<Entry>,
}

impl Db {
    pub fn load<T: AsRef<Path>>(path: T) -> Result<Db> {
        let mut file = File::open(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let db: Db = serde_json::from_str(&String::from_utf8(buf)?)?;
        Ok(db)
    }

    pub fn save<T: AsRef<Path>>(&self, path: T) -> Result<()> {
        let mut file = File::create(&path)?;
        let encoded: Vec<u8> = serde_json::to_string(&self)?.into_bytes();
        file.write_all(&encoded)?;
        file.flush()?;

        Ok(())
    }

    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Entry {
    #[serde(with = "ts_seconds")]
    pub date: DateTime<Utc>,
    pub sources: u64,
    pub projects: u64,
}

const DIR_PATH: &str = "statistics";
const JSON_PATH: &str = "statistics/db.json";
const SVG_PATH: &str = "statistics/plot.svg";

async fn update() -> Result<()> {
    let dir = PathBuf::from(DIR_PATH);
    let path = PathBuf::from(JSON_PATH);

    if !dir.exists() {
        std::fs::create_dir(DIR_PATH)?;
    }

    let mut db = if path.exists() {
        Db::load(&path)?
    } else {
        Db::default()
    };

    let token = SecretString::from_str(&std::env::var("GITHUB_TOKEN").unwrap())?;
    let octocrab = octocrab::Octocrab::builder()
        .personal_token(token)
        .build()?;

    let page = octocrab.search().code("extension:veryl").send().await?;
    let sources = page.total_count.unwrap_or(0);

    let page = octocrab.search().code("filename:Veryl.toml").send().await?;
    let projects = page.total_count.unwrap_or(0);

    let entry = Entry {
        date: Utc::now(),
        sources,
        projects,
    };

    db.push(entry);
    db.save(&path)?;

    Ok(())
}

fn plot() -> Result<()> {
    let dir = PathBuf::from(DIR_PATH);
    let path = PathBuf::from(JSON_PATH);

    if !dir.exists() {
        std::fs::create_dir(DIR_PATH)?;
    }

    let db = if path.exists() {
        Db::load(&path)?
    } else {
        Db::default()
    };

    let mut src_plot = Vec::new();
    let mut prj_plot = Vec::new();
    let mut x_min = Utc
        .timestamp_opt(std::i32::MAX as i64, 0)
        .unwrap()
        .date_naive();
    let mut x_max = Utc.timestamp_opt(0, 0).unwrap().date_naive();
    let mut src_max = 0;
    let mut prj_max = 0;

    for entry in &db.entries {
        let x_val = entry.date.date_naive();

        x_min = x_min.min(x_val);
        x_max = x_max.max(x_val);
        src_max = src_max.max(entry.sources);
        prj_max = prj_max.max(entry.projects);

        src_plot.push((x_val, entry.sources));
        prj_plot.push((x_val, entry.projects));
    }

    src_max *= 2;
    prj_max *= 2;

    let backend = SVGBackend::new(SVG_PATH, (1200, 800));
    let root = backend.into_drawing_area();
    let _ = root.fill(&WHITE);
    let root = root.margin(10, 10, 10, 10);
    let mut chart = ChartBuilder::on(&root)
        .x_label_area_size(50)
        .y_label_area_size(50)
        .right_y_label_area_size(50)
        .build_cartesian_2d(x_min..x_max, 0..src_max)?
        .set_secondary_coord(x_min..x_max, 0..prj_max);

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_y_mesh()
        .y_label_formatter(&|x| format!("{}", x))
        .y_desc("Source")
        .draw()?;

    chart.configure_secondary_axes().y_desc("Project").draw()?;

    let src_style = ShapeStyle {
        color: GREEN.into(),
        filled: true,
        stroke_width: 2,
    };

    let prj_style = ShapeStyle {
        color: BLUE.into(),
        filled: true,
        stroke_width: 2,
    };

    let anno = chart.draw_series(LineSeries::new(src_plot, src_style.clone()))?;
    anno.label("source").legend(move |(x, y)| {
        plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], src_style.clone())
    });
    let anno = chart.draw_secondary_series(LineSeries::new(prj_plot, prj_style.clone()))?;
    anno.label("project").legend(move |(x, y)| {
        plotters::prelude::PathElement::new(vec![(x, y), (x + 20, y)], prj_style.clone())
    });

    chart
        .configure_series_labels()
        .position(SeriesLabelPosition::UpperLeft)
        .background_style(&WHITE)
        .border_style(&BLACK)
        .draw()?;

    chart.plotting_area().present()?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = update().await?;
    plot()?;

    Ok(())
}
