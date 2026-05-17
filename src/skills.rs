include!(concat!(env!("OUT_DIR"), "/skills_bundle.rs"));

pub fn list_skills() -> String {
    if SKILLS.is_empty() {
        return "No skills are installed. The skills/ directory is empty or missing.".to_string();
    }
    let mut out = String::from("Available skills:\n");
    for (name, description, _) in SKILLS {
        out.push_str(&format!("- {name}: {description}\n"));
    }
    out.push_str(
        "\nTo load a skill and use its full instructions, call <readSkill name=\"skill-name\"/>.",
    );
    out
}

pub fn read_skill(name: &str) -> String {
    for (skill_name, _, content) in SKILLS {
        if *skill_name == name {
            return content.to_string();
        }
    }
    let available: Vec<&str> = SKILLS.iter().map(|(n, _, _)| *n).collect();
    if available.is_empty() {
        format!("Skill '{name}' not found. No skills are currently installed.")
    } else {
        format!(
            "Skill '{name}' not found. Available skills: {}",
            available.join(", ")
        )
    }
}
