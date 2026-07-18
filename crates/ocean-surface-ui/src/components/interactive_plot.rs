//! Locally reactive, bounded plots driven by declarative math expressions.

use std::collections::{BTreeMap, BTreeSet};

use leptos::prelude::*;
use serde_json::{json, Value};

use crate::daemon::Daemon;

const MAX_PARAMETERS: usize = 12;
const MAX_SERIES: usize = 6;
const MAX_METRICS: usize = 12;
const MAX_SAMPLES: usize = 512;
const MAX_EXPRESSION_LEN: usize = 512;

#[derive(Clone, Debug)]
struct PlotConfig {
    title: String,
    description: String,
    parameters: Vec<Parameter>,
    x: Axis,
    y_label: String,
    series: Vec<Series>,
    metrics: Vec<Metric>,
}

#[derive(Clone, Debug)]
struct Parameter {
    id: String,
    label: String,
    min: f64,
    max: f64,
    step: f64,
    value: f64,
    unit: String,
}

#[derive(Clone, Debug)]
struct Axis {
    id: String,
    label: String,
    min: f64,
    max: f64,
    samples: usize,
}

#[derive(Clone, Debug)]
struct Series {
    label: String,
    expression: Expr,
}

#[derive(Clone, Debug)]
struct Metric {
    label: String,
    expression: Expr,
    unit: String,
    precision: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Expr {
    Number(f64),
    Variable(String),
    Unary(char, Box<Expr>),
    Binary(char, Box<Expr>, Box<Expr>),
    Function(String, Vec<Expr>),
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    fn parse(source: &'a str) -> Result<Expr, String> {
        if source.trim().is_empty() {
            return Err("expression is empty".into());
        }
        if source.len() > MAX_EXPRESSION_LEN {
            return Err(format!(
                "expression exceeds {MAX_EXPRESSION_LEN} characters"
            ));
        }
        let mut parser = Self { source, offset: 0 };
        let expression = parser.parse_additive()?;
        parser.skip_space();
        if parser.offset != source.len() {
            return Err(format!(
                "unexpected input at character {}",
                parser.offset + 1
            ));
        }
        Ok(expression)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            self.skip_space();
            let Some(operator @ ('+' | '-')) = self.peek() else {
                return Ok(left);
            };
            self.bump();
            left = Expr::Binary(
                operator,
                Box::new(left),
                Box::new(self.parse_multiplicative()?),
            );
        }
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_space();
            let Some(operator @ ('*' | '/')) = self.peek() else {
                return Ok(left);
            };
            self.bump();
            left = Expr::Binary(operator, Box::new(left), Box::new(self.parse_unary()?));
        }
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        self.skip_space();
        if let Some(operator @ ('+' | '-')) = self.peek() {
            self.bump();
            return Ok(Expr::Unary(operator, Box::new(self.parse_unary()?)));
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> Result<Expr, String> {
        let left = self.parse_primary()?;
        self.skip_space();
        if self.peek() == Some('^') {
            self.bump();
            return Ok(Expr::Binary(
                '^',
                Box::new(left),
                Box::new(self.parse_unary()?),
            ));
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        self.skip_space();
        match self.peek() {
            Some('(') => {
                self.bump();
                let expression = self.parse_additive()?;
                self.skip_space();
                if self.bump() != Some(')') {
                    return Err("missing closing parenthesis".into());
                }
                Ok(expression)
            }
            Some(ch) if ch.is_ascii_digit() || ch == '.' => self.parse_number(),
            Some(ch) if is_identifier_start(ch) => self.parse_name(),
            Some(ch) => Err(format!(
                "unexpected '{ch}' at character {}",
                self.offset + 1
            )),
            None => Err("unexpected end of expression".into()),
        }
    }

    fn parse_number(&mut self) -> Result<Expr, String> {
        let start = self.offset;
        let mut saw_digit = false;
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            saw_digit = true;
            self.bump();
        }
        if self.peek() == Some('.') {
            self.bump();
            while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                saw_digit = true;
                self.bump();
            }
        }
        if !saw_digit {
            return Err(format!("invalid number at character {}", start + 1));
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            let exponent_start = self.offset;
            while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                self.bump();
            }
            if exponent_start == self.offset {
                return Err("invalid numeric exponent".into());
            }
        }
        let number = self.source[start..self.offset]
            .parse::<f64>()
            .map_err(|_| "invalid number".to_string())?;
        if !number.is_finite() {
            return Err("number is not finite".into());
        }
        Ok(Expr::Number(number))
    }

    fn parse_name(&mut self) -> Result<Expr, String> {
        let start = self.offset;
        self.bump();
        while self.peek().is_some_and(is_identifier_continue) {
            self.bump();
        }
        let name = self.source[start..self.offset].to_ascii_lowercase();
        self.skip_space();
        if self.peek() != Some('(') {
            return Ok(Expr::Variable(name));
        }
        self.bump();
        let mut arguments = Vec::new();
        self.skip_space();
        if self.peek() != Some(')') {
            loop {
                arguments.push(self.parse_additive()?);
                self.skip_space();
                match self.peek() {
                    Some(',') => {
                        self.bump();
                    }
                    Some(')') => break,
                    _ => return Err(format!("expected ',' or ')' in {name}(...)")),
                }
            }
        }
        self.bump();
        let expected = match name.as_str() {
            "min" | "max" => 2,
            "sin" | "cos" | "tan" | "exp" | "ln" | "log" | "sqrt" | "abs" => 1,
            _ => return Err(format!("unknown function '{name}'")),
        };
        if arguments.len() != expected {
            return Err(format!("{name} expects {expected} argument(s)"));
        }
        Ok(Expr::Function(name, arguments))
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        Some(ch)
    }
}

impl Expr {
    fn evaluate(&self, variables: &BTreeMap<String, f64>) -> Result<f64, String> {
        let value = match self {
            Self::Number(value) => *value,
            Self::Variable(name) if name == "pi" => std::f64::consts::PI,
            Self::Variable(name) if name == "e" => std::f64::consts::E,
            Self::Variable(name) => *variables
                .get(name)
                .ok_or_else(|| format!("unknown variable '{name}'"))?,
            Self::Unary('+', expression) => expression.evaluate(variables)?,
            Self::Unary('-', expression) => -expression.evaluate(variables)?,
            Self::Unary(operator, _) => return Err(format!("unknown unary operator '{operator}'")),
            Self::Binary(operator, left, right) => {
                let left = left.evaluate(variables)?;
                let right = right.evaluate(variables)?;
                match operator {
                    '+' => left + right,
                    '-' => left - right,
                    '*' => left * right,
                    '/' => left / right,
                    '^' => left.powf(right),
                    _ => return Err(format!("unknown operator '{operator}'")),
                }
            }
            Self::Function(name, arguments) => {
                let values = arguments
                    .iter()
                    .map(|argument| argument.evaluate(variables))
                    .collect::<Result<Vec<_>, _>>()?;
                match name.as_str() {
                    "sin" => values[0].sin(),
                    "cos" => values[0].cos(),
                    "tan" => values[0].tan(),
                    "exp" => values[0].exp(),
                    "ln" | "log" => values[0].ln(),
                    "sqrt" => values[0].sqrt(),
                    "abs" => values[0].abs(),
                    "min" => values[0].min(values[1]),
                    "max" => values[0].max(values[1]),
                    _ => return Err(format!("unknown function '{name}'")),
                }
            }
        };
        value
            .is_finite()
            .then_some(value)
            .ok_or_else(|| "expression produced a non-finite value".into())
    }
}

fn is_identifier_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_identifier_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars.next().is_some_and(is_identifier_start) && chars.all(is_identifier_continue)
}

impl PlotConfig {
    fn parse(props: &Value) -> Result<Self, String> {
        let parameters_json = props
            .get("parameters")
            .and_then(Value::as_array)
            .ok_or("parameters must be an array")?;
        if parameters_json.len() > MAX_PARAMETERS {
            return Err(format!("at most {MAX_PARAMETERS} parameters are supported"));
        }

        let plot = props.get("plot").ok_or("plot is required")?;
        let x_json = plot.get("x").ok_or("plot.x is required")?;
        let x_id = text(x_json, "id").unwrap_or_else(|| "x".into());
        if x_id != x_id.to_ascii_lowercase()
            || !valid_identifier(&x_id)
            || matches!(x_id.as_str(), "pi" | "e")
        {
            return Err("plot.x.id must be a lowercase ASCII non-reserved variable name".into());
        }
        let x_min = finite_number(x_json, "min")?;
        let x_max = finite_number(x_json, "max")?;
        if x_min >= x_max {
            return Err("plot.x.min must be less than plot.x.max".into());
        }
        let samples = x_json
            .get("samples")
            .and_then(Value::as_u64)
            .unwrap_or(128)
            .clamp(2, MAX_SAMPLES as u64) as usize;
        let x = Axis {
            id: x_id.clone(),
            label: text(x_json, "label").unwrap_or_else(|| x_id.clone()),
            min: x_min,
            max: x_max,
            samples,
        };

        let mut ids = BTreeSet::from([x_id, "pi".into(), "e".into()]);
        let mut parameters = Vec::with_capacity(parameters_json.len());
        for raw in parameters_json {
            let id = text(raw, "id").ok_or("every parameter requires an id")?;
            if id != id.to_ascii_lowercase() || !valid_identifier(&id) || !ids.insert(id.clone()) {
                return Err(format!(
                    "parameter id '{id}' must be lowercase ASCII, non-reserved, and unique"
                ));
            }
            let min = finite_number(raw, "min")?;
            let max = finite_number(raw, "max")?;
            if min >= max {
                return Err(format!("parameter '{id}' min must be less than max"));
            }
            let range = max - min;
            let step = raw
                .get("step")
                .and_then(Value::as_f64)
                .filter(|step| step.is_finite() && *step > 0.0)
                .unwrap_or(range / 100.0)
                .min(range);
            let value = raw
                .get("value")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or(min)
                .clamp(min, max);
            parameters.push(Parameter {
                label: text(raw, "label").unwrap_or_else(|| id.clone()),
                unit: text(raw, "unit").unwrap_or_default(),
                id,
                min,
                max,
                step,
                value,
            });
        }

        let series_json = plot
            .get("series")
            .and_then(Value::as_array)
            .ok_or("plot.series must be an array")?;
        if series_json.is_empty() || series_json.len() > MAX_SERIES {
            return Err(format!(
                "plot.series must contain 1 to {MAX_SERIES} entries"
            ));
        }
        let mut series = Vec::with_capacity(series_json.len());
        for (index, raw) in series_json.iter().enumerate() {
            let source = text(raw, "expression")
                .ok_or_else(|| format!("series {} requires an expression", index + 1))?;
            series.push(Series {
                label: text(raw, "label").unwrap_or_else(|| format!("Series {}", index + 1)),
                expression: Parser::parse(&source)
                    .map_err(|error| format!("series {}: {error}", index + 1))?,
            });
        }

        let metrics_json = props
            .get("metrics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if metrics_json.len() > MAX_METRICS {
            return Err(format!("at most {MAX_METRICS} metrics are supported"));
        }
        let mut metrics = Vec::with_capacity(metrics_json.len());
        for (index, raw) in metrics_json.iter().enumerate() {
            let source = text(raw, "expression")
                .ok_or_else(|| format!("metric {} requires an expression", index + 1))?;
            metrics.push(Metric {
                label: text(raw, "label").unwrap_or_else(|| format!("Metric {}", index + 1)),
                expression: Parser::parse(&source)
                    .map_err(|error| format!("metric {}: {error}", index + 1))?,
                unit: text(raw, "unit").unwrap_or_default(),
                precision: raw
                    .get("precision")
                    .and_then(Value::as_u64)
                    .unwrap_or(2)
                    .min(6) as usize,
            });
        }

        Ok(Self {
            title: text(props, "title").unwrap_or_default(),
            description: text(props, "description").unwrap_or_default(),
            parameters,
            x,
            y_label: text(plot, "y_label").unwrap_or_default(),
            series,
            metrics,
        })
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn finite_number(value: &Value, key: &str) -> Result<f64, String> {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .ok_or_else(|| format!("{key} must be a finite number"))
}

#[derive(Debug, PartialEq)]
struct SampledPlot {
    paths: Vec<String>,
    y_min: f64,
    y_max: f64,
}

fn sample_plot(
    config: &PlotConfig,
    parameters: &BTreeMap<String, f64>,
) -> Result<SampledPlot, String> {
    let mut values = parameters.clone();
    let mut sampled = vec![Vec::with_capacity(config.x.samples); config.series.len()];
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;

    for sample in 0..config.x.samples {
        let progress = sample as f64 / (config.x.samples - 1) as f64;
        let x = config.x.min + progress * (config.x.max - config.x.min);
        values.insert(config.x.id.clone(), x);
        for (index, series) in config.series.iter().enumerate() {
            let y = series
                .expression
                .evaluate(&values)
                .map_err(|error| format!("{} could not be evaluated: {error}", series.label))?;
            y_min = y_min.min(y);
            y_max = y_max.max(y);
            sampled[index].push((progress, y));
        }
    }

    if !y_min.is_finite() || !y_max.is_finite() {
        return Err("the plot did not produce finite values".into());
    }
    if (y_max - y_min).abs() < f64::EPSILON {
        let padding = y_max.abs().max(1.0) * 0.05;
        let padded_min = y_min - padding;
        let padded_max = y_max + padding;
        if !padded_min.is_finite() || !padded_max.is_finite() {
            return Err("the plot range is too large to normalize".into());
        }
        y_min = padded_min;
        y_max = padded_max;
    }
    let span = y_max - y_min;
    if !span.is_finite() || span <= 0.0 {
        return Err("the plot range is too large to normalize".into());
    }
    let paths = sampled
        .into_iter()
        .map(|points| {
            points
                .into_iter()
                .enumerate()
                .map(|(index, (x, y))| {
                    let command = if index == 0 { 'M' } else { 'L' };
                    let plot_x = x * 120.0;
                    let plot_y = 60.0 - (y - y_min) / span * 60.0;
                    if !plot_x.is_finite() || !plot_y.is_finite() {
                        return Err("the plot produced invalid normalized geometry".to_string());
                    }
                    Ok(format!("{command}{plot_x:.3},{plot_y:.3}"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|segments| segments.join(" "))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SampledPlot {
        paths,
        y_min,
        y_max,
    })
}

fn update_parameter(
    values: RwSignal<BTreeMap<String, f64>>,
    parameter: &Parameter,
    raw: &str,
) -> Option<f64> {
    let value = raw.parse::<f64>().ok()?;
    if !value.is_finite() {
        return None;
    }
    let value = value.clamp(parameter.min, parameter.max);
    values.update(|current| {
        current.insert(parameter.id.clone(), value);
    });
    Some(value)
}

fn format_value(value: f64, precision: usize) -> String {
    let rendered = format!("{value:.precision$}");
    let Some((whole, fraction)) = rendered.split_once('.') else {
        return rendered;
    };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{fraction}")
    }
}

#[component]
pub(super) fn InteractivePlotView(
    component_id: String,
    kind_props: Value,
    daemon: Daemon,
) -> impl IntoView {
    let config = PlotConfig::parse(&kind_props);
    let Ok(config) = config else {
        return view! {
            <div class="component-interactive-plot component-interactive-plot--error" role="status">
                <strong>"Interactive plot unavailable"</strong>
                <span>{config.expect_err("checked above")}</span>
            </div>
        }
        .into_any();
    };
    let initial = config
        .parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.value))
        .collect::<BTreeMap<_, _>>();
    let values = RwSignal::new(initial);

    let graph_config = config.clone();
    let metric_config = config.clone();
    let title = config.title.clone();
    let aria_label = if title.is_empty() {
        "Interactive plot".to_string()
    } else {
        title.clone()
    };
    let heading = title.clone();
    let description = config.description.clone();
    let controls = config.parameters.clone();

    view! {
        <section class="component-interactive-plot" aria-label=aria_label>
            {(!title.is_empty()).then(|| view! { <h3 class="interactive-plot__title">{heading}</h3> })}
            {(!description.is_empty()).then(|| view! { <p class="interactive-plot__description">{description}</p> })}
            <div class="interactive-plot__graph">
                {move || match sample_plot(&graph_config, &values.get()) {
                    Ok(plot) => {
                        let y_max = format_value(plot.y_max, 2);
                        let y_min = format_value(plot.y_min, 2);
                        let x_min = format_value(graph_config.x.min, 2);
                        let x_max = format_value(graph_config.x.max, 2);
                        let paths = plot.paths.into_iter().enumerate().map(|(index, path)| view! {
                            <path class=format!("interactive-plot__line interactive-plot__line--{}", index + 1) d=path></path>
                        }).collect::<Vec<_>>();
                        let series_labels = graph_config.series.iter()
                            .map(|series| series.label.clone())
                            .collect::<Vec<_>>();
                        let plot_label = format!("Computed plot with series: {}", series_labels.join(", "));
                        let legend = series_labels.into_iter().enumerate().map(|(index, label)| view! {
                            <span class="interactive-plot__legend-item">
                                <i
                                    class=format!("interactive-plot__legend-swatch interactive-plot__legend-swatch--{}", index + 1)
                                    aria-hidden="true"
                                ></i>
                                <span>{label}</span>
                            </span>
                        }).collect::<Vec<_>>();
                        view! {
                            <div class="interactive-plot__y-label">{graph_config.y_label.clone()}</div>
                            <div class="interactive-plot__plot-area">
                                <span class="interactive-plot__y-max">{y_max}</span>
                                <svg viewBox="0 0 120 60" preserveAspectRatio="none" role="img" aria-label=plot_label>
                                    <line class="interactive-plot__grid" x1="0" y1="30" x2="120" y2="30"></line>
                                    {paths}
                                </svg>
                                <span class="interactive-plot__y-min">{y_min}</span>
                            </div>
                            <div class="interactive-plot__x-axis">
                                <span>{x_min}</span>
                                <span>{graph_config.x.label.clone()}</span>
                                <span>{x_max}</span>
                            </div>
                            <div class="interactive-plot__legend" aria-label="Plot series">{legend}</div>
                        }.into_any()
                    }
                    Err(error) => view! {
                        <div class="interactive-plot__error" role="status">{error}</div>
                    }.into_any(),
                }}
            </div>

            {(!metric_config.metrics.is_empty()).then(|| view! {
                <div class="interactive-plot__metrics" aria-label="Derived metrics">
                    {move || {
                        let mut variables = values.get();
                        variables.insert(metric_config.x.id.clone(), metric_config.x.min);
                        metric_config.metrics.iter().map(|metric| {
                            let rendered = metric.expression.evaluate(&variables)
                                .map(|value| format!("{}{}", format_value(value, metric.precision), metric.unit))
                                .unwrap_or_else(|_| "Unavailable".into());
                            view! {
                                <div class="interactive-plot__metric">
                                    <span>{metric.label.clone()}</span>
                                    <strong>{rendered}</strong>
                                </div>
                            }
                        }).collect::<Vec<_>>()
                    }}
                </div>
            })}

            {(!controls.is_empty()).then(|| view! {
                <div class="interactive-plot__controls" aria-label="Plot parameters">
                    {controls.into_iter().map(|parameter| {
                        let range_parameter = parameter.clone();
                        let number_parameter = parameter.clone();
                        let commit_parameter = parameter.clone();
                        let number_commit_parameter = parameter.clone();
                        let range_id = format!("interactive-plot-{}-{}", component_id, parameter.id);
                        let number_id = format!("{range_id}-number");
                        let number_label_id = number_id.clone();
                        let component_id_range = component_id.clone();
                        let component_id_number = component_id.clone();
                        let daemon_range = daemon.clone();
                        let daemon_number = daemon.clone();
                        let value_id = parameter.id.clone();
                        let number_value_id = parameter.id.clone();
                        let unit = parameter.unit.clone();
                        view! {
                            <fieldset class="interactive-plot__control">
                                <legend>{parameter.label.clone()}</legend>
                                <div class="interactive-plot__control-row">
                                    <input
                                        id=range_id
                                        class="interactive-plot__range"
                                        type="range"
                                        min=parameter.min.to_string()
                                        max=parameter.max.to_string()
                                        step=parameter.step.to_string()
                                        prop:value=move || values.with(|current| current.get(&value_id).copied().unwrap_or(range_parameter.value).to_string())
                                        aria-label=format!("{} slider", parameter.label)
                                        on:input=move |event| { update_parameter(values, &range_parameter, &event_target_value(&event)); }
                                        on:change=move |event| {
                                            if let Some(value) = update_parameter(values, &commit_parameter, &event_target_value(&event)) {
                                                let parameters = values.get_untracked();
                                                daemon_range.send_component_event(component_id_range.clone(), json!({
                                                    "type": "parameters_changed",
                                                    "payload": { "parameters": parameters, "changed": { "id": commit_parameter.id, "value": value } }
                                                }));
                                            }
                                        }
                                    />
                                    <label class="interactive-plot__number-wrap" for=number_label_id>
                                        <span class="interactive-plot__sr-only">{format!("{} value", parameter.label)}</span>
                                        <input
                                            id=number_id
                                            class="interactive-plot__number"
                                            type="number"
                                            min=parameter.min.to_string()
                                            max=parameter.max.to_string()
                                            step=parameter.step.to_string()
                                            prop:value=move || values.with(|current| current.get(&number_value_id).copied().unwrap_or(number_parameter.value).to_string())
                                            on:input=move |event| { update_parameter(values, &number_parameter, &event_target_value(&event)); }
                                            on:change=move |event| {
                                                if let Some(value) = update_parameter(values, &number_commit_parameter, &event_target_value(&event)) {
                                                    let parameters = values.get_untracked();
                                                    daemon_number.send_component_event(component_id_number.clone(), json!({
                                                        "type": "parameters_changed",
                                                        "payload": { "parameters": parameters, "changed": { "id": number_commit_parameter.id, "value": value } }
                                                    }));
                                                }
                                            }
                                        />
                                        {(!unit.is_empty()).then(|| view! { <span class="interactive-plot__unit">{unit}</span> })}
                                    </label>
                                </div>
                            </fieldset>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            })}
        </section>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(source: &str, variables: &[(&str, f64)]) -> Result<f64, String> {
        Parser::parse(source)?.evaluate(
            &variables
                .iter()
                .map(|(name, value)| ((*name).into(), *value))
                .collect(),
        )
    }

    #[test]
    fn expression_engine_supports_precedence_constants_functions_and_variables() {
        assert_eq!(eval("2 + 3 * 4", &[]).unwrap(), 14.0);
        assert_eq!(eval("2^3^2", &[]).unwrap(), 512.0);
        assert!((eval("sin(pi / 2) + max(m, 2)", &[("m", 3.0)]).unwrap() - 4.0).abs() < 1e-10);
        assert!(
            (eval("exp(ln(e)) + sqrt(9) + abs(-2)", &[]).unwrap() - (std::f64::consts::E + 5.0))
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn expression_engine_rejects_unknown_or_non_finite_work() {
        assert!(eval("unknown(1)", &[]).is_err());
        assert!(eval("missing + 1", &[]).is_err());
        assert!(eval("1 / 0", &[]).is_err());
        assert!(Parser::parse(&"1".repeat(MAX_EXPRESSION_LEN + 1)).is_err());
    }

    fn valid_props() -> Value {
        json!({
            "title": "Decay",
            "parameters": [{"id":"rate", "min":0.1, "max":3.0, "value":1.0}],
            "plot": {
                "x": {"min":0.0, "max":10.0, "samples":32},
                "series": [{"label":"Value", "expression":"exp(-rate*x)"}]
            },
            "metrics": [{"label":"Half life", "expression":"ln(2)/rate"}]
        })
    }

    #[test]
    fn config_clamps_values_and_sample_work() {
        let mut props = valid_props();
        props["parameters"][0]["value"] = json!(99.0);
        props["plot"]["x"]["samples"] = json!(50_000);
        let config = PlotConfig::parse(&props).unwrap();
        assert_eq!(config.parameters[0].value, 3.0);
        assert_eq!(config.x.samples, MAX_SAMPLES);
    }

    #[test]
    fn config_rejects_malformed_excessive_and_noncanonical_props() {
        assert!(PlotConfig::parse(&json!({})).is_err());
        let mut props = valid_props();
        props["parameters"] = Value::Array(
            (0..=MAX_PARAMETERS)
                .map(|index| {
                    json!({
                        "id": format!("p{index}"), "min": 0, "max": 1
                    })
                })
                .collect(),
        );
        assert!(PlotConfig::parse(&props).is_err());

        let mut props = valid_props();
        props["parameters"][0]["id"] = json!("DecayRate");
        assert!(PlotConfig::parse(&props).is_err());
    }

    #[test]
    fn format_value_preserves_integer_zeroes_at_zero_precision() {
        assert_eq!(format_value(0.0, 0), "0");
        assert_eq!(format_value(10.0, 0), "10");
        assert_eq!(format_value(100.0, 0), "100");
        assert_eq!(format_value(10.50, 2), "10.5");
    }

    #[test]
    fn sampled_plot_is_finite_and_stays_inside_viewbox() {
        let config = PlotConfig::parse(&valid_props()).unwrap();
        let parameters = BTreeMap::from([("rate".into(), 1.0)]);
        let plot = sample_plot(&config, &parameters).unwrap();
        assert_eq!(plot.paths.len(), 1);
        assert!(plot.y_min.is_finite() && plot.y_max.is_finite());
        for pair in plot.paths[0].split_whitespace() {
            let (_, coordinates) = pair.split_at(1);
            let (x, y) = coordinates.split_once(',').unwrap();
            assert!((0.0..=120.0).contains(&x.parse::<f64>().unwrap()));
            assert!((0.0..=60.0).contains(&y.parse::<f64>().unwrap()));
        }
    }

    #[test]
    fn sampled_plot_rejects_extreme_ranges_that_overflow_normalization() {
        let mut props = valid_props();
        props["plot"]["x"]["min"] = json!(-1.0);
        props["plot"]["x"]["max"] = json!(1.0);
        props["plot"]["series"] = json!([
            {"label":"Extreme", "expression":"x*1e308"}
        ]);
        let config = PlotConfig::parse(&props).unwrap();
        let parameters = BTreeMap::from([("rate".into(), 1.0)]);
        let error = sample_plot(&config, &parameters).unwrap_err();
        assert!(error.contains("too large") || error.contains("invalid normalized"));
    }
}
