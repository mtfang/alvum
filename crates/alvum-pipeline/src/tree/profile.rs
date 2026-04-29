//! Prompt-context helpers for the user-managed synthesis profile.

use alvum_core::pipeline_events::{self as events, Event};
use alvum_core::synthesis_profile::SynthesisProfile;
use alvum_core::util::defang_wrapper_tag;
use anyhow::Result;

use super::level::LevelContextBlock;

pub const PROFILE_TAG: &str = "synthesis_profile";
pub const ADVANCED_TAG: &str = "user_synthesis_instructions";

pub fn context_blocks(
    profile: &SynthesisProfile,
    include_advanced: bool,
) -> Result<Vec<LevelContextBlock>> {
    let mut blocks = vec![LevelContextBlock {
        tag: PROFILE_TAG,
        body: profile.prompt_profile_json()?,
    }];
    if include_advanced && let Some(instructions) = profile.prompt_advanced_instructions() {
        blocks.push(LevelContextBlock {
            tag: ADVANCED_TAG,
            body: instructions,
        });
    }
    Ok(blocks)
}

pub fn append_blocks(
    user_message: &mut String,
    stage: &str,
    profile: &SynthesisProfile,
    include_advanced: bool,
) -> Result<()> {
    append_block(
        user_message,
        stage,
        PROFILE_TAG,
        &profile.prompt_profile_json()?,
    );
    if include_advanced && let Some(instructions) = profile.prompt_advanced_instructions() {
        append_block(user_message, stage, ADVANCED_TAG, &instructions);
    }
    Ok(())
}

fn append_block(user_message: &mut String, stage: &str, tag: &str, body: &str) {
    let (safe, defanged) = defang_wrapper_tag(body, tag);
    if defanged > 0 {
        events::emit(Event::InputFiltered {
            processor: format!("{stage}/{tag}-wrapper-guard"),
            file: None,
            kept: body.len(),
            dropped: 0,
            reasons: serde_json::json!({"wrapper_breakout_defanged": defanged}),
        });
    }
    user_message.push_str(&format!("<{tag}>\n{safe}\n</{tag}>\n\n"));
}
