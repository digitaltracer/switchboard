//! Answers to Claude's AskUserQuestion and ExitPlanMode through its PermissionRequest hook.
//! Options are picked by id, `<question>.<option>`, so labels that mask alike stay distinct.
//! A plan needs its own input back as `updatedInput`: Claude ignores a plain allow on one.

use serde_json::{Map, Value};

/// The one option of an allowed plan.
pub const PLAN_OPTION: &str = "allow";
pub const PLAN_LABEL: &str = "Approve";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub question: String,
    pub header: Option<String>,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The questions of an AskUserQuestion input; empty for anything else.
pub fn questions(tool_input: &Value) -> Vec<Question> {
    let Some(qs) = tool_input.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    qs.iter()
        .enumerate()
        .filter_map(|(qi, q)| {
            let options = q
                .get("options")?
                .as_array()?
                .iter()
                .enumerate()
                .map(|(oi, o)| {
                    Some(QuestionOption {
                        id: format!("{qi}.{oi}"),
                        label: text(o, "label")?,
                        description: text(o, "description"),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Question {
                question: text(q, "question")?,
                header: text(q, "header"),
                multi_select: q.get("multiSelect").and_then(Value::as_bool) == Some(true),
                options,
            })
        })
        .collect()
}

/// The labels `choices` pick, in order, after checking every question gets its answer.
pub fn chosen_labels(
    tool_name: &str,
    tool_input: &Value,
    choices: &[String],
) -> Result<Vec<String>, String> {
    match tool_name {
        "AskUserQuestion" => {
            let qs = questions(tool_input);
            let expected = tool_input
                .get("questions")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if qs.is_empty() || qs.len() != expected {
                return Err("the question's options can't be read".into());
            }
            let mut picked: Vec<Vec<usize>> = vec![Vec::new(); qs.len()];
            for c in choices {
                let (qi, oi) = c
                    .split_once('.')
                    .and_then(|(q, o)| Some((q.parse::<usize>().ok()?, o.parse::<usize>().ok()?)))
                    .filter(|&(q, o)| q < qs.len() && o < qs[q].options.len())
                    .ok_or_else(|| format!("unknown option {c:?}"))?;
                if picked[qi].contains(&oi) {
                    return Err(format!("option {c:?} chosen twice"));
                }
                picked[qi].push(oi);
            }
            let mut labels = Vec::new();
            for (q, p) in qs.iter().zip(&picked) {
                if p.is_empty() || (!q.multi_select && p.len() > 1) {
                    return Err(format!("question {:?} needs one answer", q.question));
                }
                labels.extend(p.iter().map(|&o| q.options[o].label.clone()));
            }
            Ok(labels)
        }
        "ExitPlanMode" if choices.is_empty() || choices == [PLAN_OPTION] => {
            Ok(vec![PLAN_LABEL.to_owned()])
        }
        "ExitPlanMode" => Err("a plan is approved or denied, with no options".into()),
        _ if choices.is_empty() => Ok(Vec::new()),
        _ => Err("a permission is approved or denied, with no options".into()),
    }
}

/// `updatedInput` for an allowed hook call, from the raw input the runner holds.
pub fn updated_input(
    tool_name: &str,
    tool_input: &Value,
    choices: &[String],
) -> Result<Option<Value>, String> {
    chosen_labels(tool_name, tool_input, choices)?;
    match tool_name {
        "AskUserQuestion" => {
            let qs = questions(tool_input);
            let mut answers = Map::new();
            for (qi, q) in qs.iter().enumerate() {
                let labels: Vec<&str> = q
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(oi, _)| choices.contains(&format!("{qi}.{oi}")))
                    .map(|(_, o)| o.label.as_str())
                    .collect();
                answers.insert(q.question.clone(), Value::String(labels.join(", ")));
            }
            let mut out = tool_input.clone();
            out["answers"] = Value::Object(answers);
            Ok(Some(out))
        }
        "ExitPlanMode" => Ok(Some(tool_input.clone())),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn input() -> Value {
        json!({"questions": [
            {"question": "Size?", "header": "Size", "multiSelect": false,
             "options": [{"label": "Small", "description": "s"}, {"label": "Large"}]},
            {"question": "Toppings?", "multiSelect": true,
             "options": [{"label": "Cheese"}, {"label": "Olives"}, {"label": "Basil"}]},
        ]})
    }

    #[test]
    fn answers_every_question_by_option_id() {
        let choices = ["0.1", "1.0", "1.2"].map(String::from);
        let out = updated_input("AskUserQuestion", &input(), &choices)
            .unwrap()
            .unwrap();
        assert_eq!(
            out["answers"],
            json!({"Size?": "Large", "Toppings?": "Cheese, Basil"})
        );
        assert_eq!(out["questions"], input()["questions"]);
        let qs = questions(&input());
        assert_eq!(qs[0].options[1].id, "0.1");
        assert!(qs[1].multi_select && !qs[0].multi_select);
    }

    #[test]
    fn every_question_needs_an_answer_and_single_select_only_one() {
        for bad in [
            vec!["0.0"],
            vec!["0.0", "0.1", "1.0"],
            vec!["0.0", "1.0", "1.0"],
            vec!["0.2", "1.0"],
            vec!["2.0", "0.0", "1.0"],
            vec!["x", "0.0", "1.0"],
        ] {
            let choices: Vec<String> = bad.iter().map(|s| s.to_string()).collect();
            assert!(
                updated_input("AskUserQuestion", &input(), &choices).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_plan_gets_its_own_input_and_a_permission_none() {
        let plan = json!({"plan": "1. do it", "planFilePath": "/p.md"});
        assert_eq!(
            updated_input("ExitPlanMode", &plan, &[]).unwrap(),
            Some(plan.clone())
        );
        assert_eq!(
            chosen_labels("ExitPlanMode", &plan, &["allow".into()]).unwrap(),
            ["Approve"]
        );
        assert!(updated_input("ExitPlanMode", &plan, &["0.0".into()]).is_err());
        assert_eq!(updated_input("Bash", &json!({}), &[]).unwrap(), None);
        assert!(updated_input("Bash", &json!({}), &["0.0".into()]).is_err());
    }
}
