//! Forms plugins ask inside tool calls (`prompt.form`): a few questions with
//! options to pick, free text, or both. The app draws the form and keeps the
//! answers while the user moves between questions; the plugin gets all of
//! them in one `prompt.result`.

use gpui::SharedString;
use serde_json::{Value, json};

pub const MAX_QUESTIONS: usize = 20;
pub const MAX_OPTIONS: usize = 20;

#[derive(Debug, PartialEq)]
pub struct PromptForm {
    pub questions: Vec<FormQuestion>,
}

#[derive(Debug, PartialEq)]
pub struct FormQuestion {
    /// Echoed in the answer; the question's position when the plugin gave none.
    pub id: SharedString,
    pub title: SharedString,
    pub description: Option<SharedString>,
    /// Empty for a question answered with text only.
    pub options: Vec<FormOption>,
    /// Several options can be picked.
    pub multiple: bool,
    /// The user can type an answer of their own. Always true without options.
    pub allow_text: bool,
    /// The form does not move on until this question has an answer.
    pub required: bool,
    pub placeholder: SharedString,
}

#[derive(Debug, PartialEq)]
pub struct FormOption {
    pub label: SharedString,
    pub description: Option<SharedString>,
}

/// What the user gave for one question.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormAnswer {
    /// Picked options, ascending.
    pub selected: Vec<usize>,
    pub text: String,
}

impl FormQuestion {
    pub fn answered(&self, answer: &FormAnswer) -> bool {
        !answer.selected.is_empty() || (self.allow_text && !answer.text.trim().is_empty())
    }
}

impl PromptForm {
    pub fn parse(params: &Value) -> Result<Self, String> {
        let list = params["questions"]
            .as_array()
            .filter(|list| !list.is_empty())
            .ok_or("prompt.form needs a non-empty questions array")?;
        if list.len() > MAX_QUESTIONS {
            return Err(format!(
                "prompt.form takes at most {MAX_QUESTIONS} questions"
            ));
        }
        let mut questions = Vec::with_capacity(list.len());
        for (index, question) in list.iter().enumerate() {
            let title = text(&question["title"])
                .ok_or_else(|| format!("question {} needs a title", index + 1))?;
            let options: Vec<FormOption> = question["options"]
                .as_array()
                .map(|options| options.iter().filter_map(parse_option).collect())
                .unwrap_or_default();
            if options.len() > MAX_OPTIONS {
                return Err(format!(
                    "question {} has more than {MAX_OPTIONS} options",
                    index + 1
                ));
            }
            let allow_text = options.is_empty() || question["allowText"].as_bool().unwrap_or(false);
            questions.push(FormQuestion {
                id: text(&question["id"]).unwrap_or_else(|| index.to_string().into()),
                title,
                description: text(&question["description"]),
                multiple: !options.is_empty() && question["multiple"].as_bool().unwrap_or(false),
                allow_text,
                required: question["required"].as_bool().unwrap_or(true),
                placeholder: text(&question["placeholder"]).unwrap_or_else(|| {
                    if options.is_empty() {
                        "Type your answer".into()
                    } else {
                        "Other".into()
                    }
                }),
                options,
            });
        }
        Ok(Self { questions })
    }

    /// The `answers` array of `prompt.result`: one entry per question, in
    /// order. A question without an answer has an empty `selected` and no
    /// `text`.
    pub fn result(&self, answers: &[FormAnswer]) -> Value {
        let empty = FormAnswer::default();
        Value::Array(
            self.questions
                .iter()
                .enumerate()
                .map(|(index, question)| {
                    let answer = answers.get(index).unwrap_or(&empty);
                    let selected: Vec<usize> = answer
                        .selected
                        .iter()
                        .copied()
                        .filter(|option| *option < question.options.len())
                        .collect();
                    let labels: Vec<&str> = selected
                        .iter()
                        .map(|option| question.options[*option].label.as_ref())
                        .collect();
                    let mut entry = json!({
                        "id": question.id.as_ref(),
                        "selected": selected,
                        "labels": labels,
                    });
                    let typed = answer.text.trim();
                    if question.allow_text && !typed.is_empty() {
                        entry["text"] = json!(typed);
                    }
                    entry
                })
                .collect(),
        )
    }
}

fn text(value: &Value) -> Option<SharedString> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| SharedString::from(text.to_string()))
}

/// An option is a string or `{ label, description? }`.
fn parse_option(value: &Value) -> Option<FormOption> {
    if let Some(label) = text(value) {
        return Some(FormOption {
            label,
            description: None,
        });
    }
    Some(FormOption {
        label: text(&value["label"])?,
        description: text(&value["description"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> PromptForm {
        PromptForm::parse(&json!({
            "questions": [
                {
                    "id": "db",
                    "title": "Which database?",
                    "options": [
                        { "label": "Postgres", "description": "Relational" },
                        "SQLite",
                        { "label": "  " },
                    ],
                    "allowText": true,
                },
                {
                    "title": "Which features?",
                    "options": ["Auth", "Billing", "Search"],
                    "multiple": true,
                    "required": false,
                },
                { "title": "Anything else?", "multiple": true },
            ],
        }))
        .unwrap()
    }

    #[test]
    fn parses_questions_with_defaults() {
        let form = form();
        assert_eq!(form.questions.len(), 3);

        let database = &form.questions[0];
        assert_eq!(database.id, "db");
        assert_eq!(database.options.len(), 2);
        assert_eq!(database.options[0].description, Some("Relational".into()));
        assert_eq!(database.options[1].label, "SQLite");
        assert!(database.allow_text && database.required && !database.multiple);
        assert_eq!(database.placeholder, "Other");

        let features = &form.questions[1];
        assert_eq!(features.id, "1");
        assert!(features.multiple && !features.allow_text && !features.required);

        // No options: answered with text, and never a multiple choice.
        let other = &form.questions[2];
        assert!(other.allow_text && !other.multiple);
        assert_eq!(other.placeholder, "Type your answer");
    }

    #[test]
    fn refuses_forms_it_cannot_show() {
        assert!(PromptForm::parse(&json!({})).is_err());
        assert!(PromptForm::parse(&json!({ "questions": [] })).is_err());
        assert!(PromptForm::parse(&json!({ "questions": [{ "options": ["A"] }] })).is_err());
        let many: Vec<Value> = (0..=MAX_QUESTIONS)
            .map(|_| json!({ "title": "Q" }))
            .collect();
        assert!(PromptForm::parse(&json!({ "questions": many })).is_err());
    }

    #[test]
    fn knows_when_a_question_is_answered() {
        let form = form();
        let picked = FormAnswer {
            selected: vec![1],
            text: String::new(),
        };
        let typed = FormAnswer {
            selected: Vec::new(),
            text: " MySQL ".into(),
        };
        let blank = FormAnswer {
            selected: Vec::new(),
            text: "  ".into(),
        };
        assert!(form.questions[0].answered(&picked));
        assert!(form.questions[0].answered(&typed));
        assert!(!form.questions[0].answered(&blank));
        // Text does not answer a question that takes none.
        assert!(!form.questions[1].answered(&typed));
    }

    #[test]
    fn builds_one_answer_per_question() {
        let form = form();
        let answers = [
            FormAnswer {
                selected: Vec::new(),
                text: " MySQL ".into(),
            },
            FormAnswer {
                selected: vec![0, 2, 9],
                text: "ignored".into(),
            },
        ];
        assert_eq!(
            form.result(&answers),
            json!([
                { "id": "db", "selected": [], "labels": [], "text": "MySQL" },
                { "id": "1", "selected": [0, 2], "labels": ["Auth", "Search"] },
                { "id": "2", "selected": [], "labels": [] },
            ])
        );
    }
}
