#[cfg(test)]
mod tests {
    use super::super::{handle_pull_request_event, handle_workflow_run_event};
    use crate::utils::branch_filter::BranchFilter;
    use crate::webhooks::github::policy::AlertPolicy;
    use actix_web::web;

    #[test]
    fn test_pull_request_event_branch_filtering() {
        let filter = BranchFilter::new(Some("main,develop"), None).unwrap();

        // PR targeting main
        let main_pr_payload = r#"{
            "action": "opened",
            "pull_request": {
                "html_url": "http://example.com/pr/1",
                "number": 1,
                "title": "Test PR",
                "merged": false,
                "user": {"login": "testuser", "type": "User"},
                "head": {"label": "feature:new-feature"},
                "base": {"label": "origin:main"}
            },
            "repository": {"name": "test-repo", "html_url": "http://example.com"},
            "sender": {"login": "testuser"}
        }"#;

        // PR targeting feature branch
        let feature_pr_payload = r#"{
            "action": "opened",
            "pull_request": {
                "html_url": "http://example.com/pr/2",
                "number": 2,
                "title": "Test PR",
                "merged": false,
                "user": {"login": "testuser", "type": "User"},
                "head": {"label": "hotfix:bug-fix"},
                "base": {"label": "origin:feature/xyz"}
            },
            "repository": {"name": "test-repo", "html_url": "http://example.com"},
            "sender": {"login": "testuser"}
        }"#;

        let main_pr_bytes = web::Bytes::from(main_pr_payload);
        let feature_pr_bytes = web::Bytes::from(feature_pr_payload);

        // Should process PR to main
        let main_result =
            handle_pull_request_event(&main_pr_bytes, Some(&filter), &AlertPolicy::default());
        assert!(!main_result.is_empty());

        // Should filter out PR to feature branch
        let feature_result =
            handle_pull_request_event(&feature_pr_bytes, Some(&filter), &AlertPolicy::default());
        assert!(feature_result.is_empty());
    }

    #[test]
    fn test_workflow_run_event_branch_filtering() {
        let filter = BranchFilter::new(None, Some("dependabot/*")).unwrap();

        // Workflow on main branch
        let main_workflow_payload = r#"{
            "action": "completed",
            "workflow_run": {
                "name": "CI",
                "html_url": "http://example.com/run/1",
                "status": "completed",
                "conclusion": "success",
                "head_branch": "main",
                "run_number": 1
            },
            "repository": {"name": "test-repo", "html_url": "http://example.com"},
            "sender": {"login": "testuser"}
        }"#;

        // Workflow on dependabot branch
        let dependabot_workflow_payload = r#"{
            "action": "completed",
            "workflow_run": {
                "name": "CI",
                "html_url": "http://example.com/run/2", 
                "status": "completed",
                "conclusion": "success",
                "head_branch": "dependabot/npm_and_yarn/lodash-4.17.21",
                "run_number": 2
            },
            "repository": {"name": "test-repo", "html_url": "http://example.com"},
            "sender": {"login": "dependabot[bot]"}
        }"#;

        let main_workflow_bytes = web::Bytes::from(main_workflow_payload);
        let dependabot_workflow_bytes = web::Bytes::from(dependabot_workflow_payload);

        // Should process workflow on main
        let main_result = handle_workflow_run_event(&main_workflow_bytes, Some(&filter));
        assert!(!main_result.is_empty());

        // Should filter out dependabot workflow
        let dependabot_result =
            handle_workflow_run_event(&dependabot_workflow_bytes, Some(&filter));
        assert!(dependabot_result.is_empty());
    }
}
