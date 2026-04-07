use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub struct DeployPlan {
    pub backend: bool,
    pub frontend: bool,
    pub warn_missing_dist: bool,
}

/// Decide what to upload based on the project layout.
pub fn plan_deploy(cwd: &Path) -> DeployPlan {
    let has_backend = cwd.join("backend").is_dir();
    let has_dist = cwd.join("dist").is_dir();
    let has_pkg = cwd.join("package.json").is_file();
    DeployPlan {
        backend: has_backend,
        frontend: has_dist,
        warn_missing_dist: !has_dist && has_pkg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_does_nothing() {
        let p = crate::testutil::scratch("deploy-empty");
        assert_eq!(
            plan_deploy(&p),
            DeployPlan { backend: false, frontend: false, warn_missing_dist: false }
        );
    }

    #[test]
    fn agent_only_project_uploads_backend() {
        let p = crate::testutil::scratch("deploy-agent");
        std::fs::create_dir_all(p.join("backend")).unwrap();
        assert_eq!(
            plan_deploy(&p),
            DeployPlan { backend: true, frontend: false, warn_missing_dist: false }
        );
    }

    #[test]
    fn built_app_uploads_frontend() {
        let p = crate::testutil::scratch("deploy-built");
        std::fs::create_dir_all(p.join("dist")).unwrap();
        crate::testutil::touch(&p.join("package.json"));
        let plan = plan_deploy(&p);
        assert!(plan.frontend);
        assert!(!plan.warn_missing_dist);
    }

    #[test]
    fn unbuilt_app_warns_about_missing_dist() {
        let p = crate::testutil::scratch("deploy-unbuilt");
        crate::testutil::touch(&p.join("package.json"));
        let plan = plan_deploy(&p);
        assert!(!plan.frontend);
        assert!(plan.warn_missing_dist, "should warn: user forgot to run build");
    }

    #[test]
    fn no_package_json_no_warning() {
        let p = crate::testutil::scratch("deploy-nopkg");
        std::fs::create_dir_all(p.join("backend")).unwrap();
        assert!(!plan_deploy(&p).warn_missing_dist);
    }

    #[test]
    fn full_app_agent_does_everything() {
        let p = crate::testutil::scratch("deploy-full");
        std::fs::create_dir_all(p.join("backend")).unwrap();
        std::fs::create_dir_all(p.join("dist")).unwrap();
        crate::testutil::touch(&p.join("package.json"));
        assert_eq!(
            plan_deploy(&p),
            DeployPlan { backend: true, frontend: true, warn_missing_dist: false }
        );
    }
}
