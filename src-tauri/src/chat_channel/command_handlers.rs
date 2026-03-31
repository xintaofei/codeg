use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use super::manager::ChatChannelManager;
use super::types::{MessageLevel, RichMessage};
use crate::db::entities::conversation;

pub async fn handle_recent(db: &DatabaseConnection) -> RichMessage {
    let rows = match conversation::Entity::find()
        .filter(conversation::Column::DeletedAt.is_null())
        .order_by_desc(conversation::Column::CreatedAt)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return RichMessage {
                title: Some("查询失败".to_string()),
                body: e.to_string(),
                fields: Vec::new(),
                level: MessageLevel::Error,
            };
        }
    };

    let recent: Vec<_> = rows.into_iter().take(5).collect();
    if recent.is_empty() {
        return RichMessage::info("暂无会话记录").with_title("最近会话");
    }

    let mut body = String::new();
    for (i, conv) in recent.iter().enumerate() {
        let title = conv.title.as_deref().unwrap_or("(无标题)");
        let agent = &conv.agent_type;
        let time = conv.created_at.format("%m-%d %H:%M");
        body.push_str(&format!(
            "{}. [{}] {} ({})\n",
            i + 1,
            agent,
            title,
            time
        ));
    }

    RichMessage::info(body.trim_end()).with_title("最近 5 条会话")
}

pub async fn handle_search(db: &DatabaseConnection, keyword: &str) -> RichMessage {
    let rows = match conversation::Entity::find()
        .filter(conversation::Column::DeletedAt.is_null())
        .order_by_desc(conversation::Column::CreatedAt)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return RichMessage {
                title: Some("查询失败".to_string()),
                body: e.to_string(),
                fields: Vec::new(),
                level: MessageLevel::Error,
            };
        }
    };

    let keyword_lower = keyword.to_lowercase();
    let matched: Vec<_> = rows
        .into_iter()
        .filter(|c| {
            c.title
                .as_deref()
                .map(|t| t.to_lowercase().contains(&keyword_lower))
                .unwrap_or(false)
        })
        .take(10)
        .collect();

    if matched.is_empty() {
        return RichMessage::info(format!("未找到包含 \"{keyword}\" 的会话"))
            .with_title("搜索结果");
    }

    let mut body = String::new();
    for (i, conv) in matched.iter().enumerate() {
        let title = conv.title.as_deref().unwrap_or("(无标题)");
        let agent = &conv.agent_type;
        body.push_str(&format!("{}. [{}] {} (ID:{})\n", i + 1, agent, title, conv.id));
    }

    RichMessage::info(body.trim_end())
        .with_title(&format!("搜索 \"{}\" - {} 条结果", keyword, matched.len()))
}

pub async fn handle_detail(db: &DatabaseConnection, conversation_id: i32) -> RichMessage {
    let conv = match conversation::Entity::find_by_id(conversation_id)
        .filter(conversation::Column::DeletedAt.is_null())
        .one(db)
        .await
    {
        Ok(Some(c)) => c,
        Ok(None) => {
            return RichMessage::info(format!("会话 {conversation_id} 不存在"))
                .with_title("未找到");
        }
        Err(e) => {
            return RichMessage {
                title: Some("查询失败".to_string()),
                body: e.to_string(),
                fields: Vec::new(),
                level: MessageLevel::Error,
            };
        }
    };

    let title = conv.title.as_deref().unwrap_or("(无标题)");
    RichMessage::info(title)
        .with_title(&format!("会话详情 #{}", conv.id))
        .with_field("代理", &conv.agent_type)
        .with_field("状态", format!("{:?}", conv.status))
        .with_field("消息数", &conv.message_count.to_string())
        .with_field("创建时间", &conv.created_at.format("%Y-%m-%d %H:%M").to_string())
}

pub async fn handle_today(db: &DatabaseConnection) -> RichMessage {
    let now = Utc::now();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc();

    let rows = match conversation::Entity::find()
        .filter(conversation::Column::DeletedAt.is_null())
        .filter(conversation::Column::CreatedAt.gte(today_start))
        .order_by_desc(conversation::Column::CreatedAt)
        .all(db)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return RichMessage {
                title: Some("查询失败".to_string()),
                body: e.to_string(),
                fields: Vec::new(),
                level: MessageLevel::Error,
            };
        }
    };

    if rows.is_empty() {
        return RichMessage::info("今日暂无编码活动").with_title("今日活动");
    }

    // Group by agent_type
    let mut by_agent: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut titles: Vec<String> = Vec::new();
    for conv in &rows {
        *by_agent.entry(conv.agent_type.clone()).or_insert(0) += 1;
        if let Some(t) = &conv.title {
            if titles.len() < 5 {
                titles.push(t.clone());
            }
        }
    }

    let mut body = format!("会话总数: {}", rows.len());
    body.push_str("\n\n按代理:");
    for (agent, count) in &by_agent {
        body.push_str(&format!("\n  {agent} - {count} 个"));
    }

    if !titles.is_empty() {
        body.push_str("\n\n最近活动:");
        for t in &titles {
            body.push_str(&format!("\n  • {t}"));
        }
    }

    RichMessage::info(body).with_title(&format!(
        "今日活动 ({})",
        now.format("%Y-%m-%d")
    ))
}

pub async fn handle_status(manager: &ChatChannelManager) -> RichMessage {
    let statuses = manager.get_status().await;
    if statuses.is_empty() {
        return RichMessage::info("暂无活跃渠道").with_title("渠道状态");
    }

    let mut body = String::new();
    for s in &statuses {
        let icon = match s.status.as_str() {
            "connected" => "●",
            "connecting" => "◎",
            "error" => "✗",
            _ => "○",
        };
        body.push_str(&format!(
            "{} {} [{}] - {}\n",
            icon, s.name, s.channel_type, s.status
        ));
    }

    RichMessage::info(body.trim_end()).with_title("渠道状态")
}

pub fn handle_help(prefix: &str) -> RichMessage {
    RichMessage::info(format!(
        "{prefix}recent - 最近 5 条会话\n\
         {prefix}search <关键词> - 搜索会话\n\
         {prefix}detail <ID> - 会话详情\n\
         {prefix}today - 今日活动汇总\n\
         {prefix}status - 渠道连接状态\n\
         {prefix}help - 显示帮助",
    ))
    .with_title("Codeg Bot 帮助")
}
