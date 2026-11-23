//! M6 world logic scenarios.
use crate::runner::Ctx;
use anyhow::Result;
use std::time::Duration;

/// `/weather rain|thunder|clear`: begin/end rain events and the 0.01-per-tick level changes, vanilla vs Basalt.
/// Leaves the weather clear (the vanilla server is shared with other scenarios).
pub fn weather(ctx: &mut Ctx) -> Result<()> {
    let bot = ctx.bot()?;
    bot.cmd("weather clear 600s");
    // let any rain from before drain away (levels move 0.01 per tick)
    bot.wait_until(Duration::from_secs(9), |s| s.rain <= 0.01 && s.thunder <= 0.01);
    bot.st().weather_events.clear();
    bot.cmd("weather rain 600s");
    ctx.check(bot.wait_until(Duration::from_secs(5), |s| s.weather_events.iter().any(|e| e.0 == 1)), "no BEGIN_RAINING event");
    ctx.check(bot.wait_until(Duration::from_secs(9), |s| s.rain >= 0.99), format!("rain level never reached 1 (now {})", bot.st().rain));
    let (begin, levels) = { let s = bot.st(); (s.weather_events.iter().filter(|e| e.0 == 1).count(), s.weather_events.iter().filter(|e| e.0 == 7).count()) };
    ctx.check(levels >= 50, format!("only {levels} rain level events"));
    ctx.metric("rain_begin_events", begin as f64, 0.0);
    ctx.metric("rain_level_events_ge_90", (levels >= 90) as u8 as f64, 0.0);
    bot.cmd("weather thunder 600s");
    ctx.check(bot.wait_until(Duration::from_secs(9), |s| s.thunder >= 0.99), format!("thunder level never reached 1 (now {})", bot.st().thunder));
    bot.st().weather_events.clear();
    bot.cmd("weather clear 600s");
    ctx.check(bot.wait_until(Duration::from_secs(5), |s| s.weather_events.iter().any(|e| e.0 == 2)), "no END_RAINING event");
    let back_to_clear = bot.wait_until(Duration::from_secs(9), |s| s.rain <= 0.01 && s.thunder <= 0.01);
    let (rain, thunder) = { let s = bot.st(); (s.rain, s.thunder) };   // one lock: two bot.st() in one expression would deadlock
    ctx.check(back_to_clear, format!("levels did not return to 0 (rain {rain}, thunder {thunder})"));
    let ends = bot.st().weather_events.iter().filter(|e| e.0 == 2).count();
    ctx.metric("rain_end_events", ends as f64, 0.0);
    bot.cmd("weather clear 600s");
    Ok(())
}
