# Contributing to W2UOS

Thank you for your interest in contributing to W2UOS.

W2UOS is a modular, high-performance quantitative trading operating system.
We welcome contributions in the form of bug reports, feature requests, code improvements, documentation, and architectural discussions.

This document defines the contribution workflow, coding standards, and review process.

---

## 1. Code of Conduct

All contributors are expected to:

- Be respectful and constructive in all discussions.
- Avoid harassment, abuse, or discriminatory behavior.
- Focus on technical merit and project improvement.

Violations may result in rejection of contributions or permanent exclusion from the project.

---

## 2. Types of Contributions

You may contribute in any of the following ways:

- Bug reports and reproducible issue submissions
- Feature requests and architectural proposals
- Pull requests (code contributions)
- Documentation improvements
- Test cases, benchmarks, and performance optimizations
- Security reviews and vulnerability disclosures

---

## 3. Development Workflow

### 3.1 Fork and Branch

1. Fork the repository from:
   https://github.com/pb2n/W2UOS

2. Create a dedicated feature branch:

git checkout -b feature/your-feature-name

Direct commits to the main branch are not allowed.

---

### 3.2 Build and Test

Before submitting any pull request, ensure that:

cargo build --release  
cargo test

The code must compile and all tests must pass.

Generated files, secrets, and local environment overrides must not be committed.

---

### 3.3 Commit Guidelines

- Use clear, descriptive commit messages.
- Keep one logical change per commit.
- Do not mix refactoring and functional changes in a single commit.
- Do not commit private keys, API keys, or credentials.

Example format:

feat(exec): add simulated trailing stop logic  
fix(api): prevent panic on empty authorization header  
refactor(bus): simplify message routing

---

## 4. Pull Request Requirements

All pull requests must:

- Clearly describe the purpose and scope of the change.
- Reference related issues if applicable.
- Preserve backward compatibility unless discussed in advance.
- Follow the existing architecture and module boundaries.

Pull requests may be rejected if they:

- Introduce breaking changes without prior approval
- Degrade performance or system stability
- Violate security or privacy principles
- Contain unrelated bulk refactoring

---

## 5. Architectural Principles

All contributions must respect the following principles:

- Strong service decoupling via message bus
- Strict separation between simulation and live execution
- Deterministic and reproducible behavior in backtest mode
- No hard-coded exchange credentials or secrets
- Risk management must never be bypassed
- All critical logic must be testable

---

## 6. Security Reporting

If you discover a security vulnerability:

- Do NOT open a public issue.
- Report it privately to the project author via GitHub.

Public disclosure is allowed only after a patch or mitigation has been released.

---

## 7. Documentation Contributions

Contributions to the following are strongly encouraged:

- README.md
- Configuration references
- API documentation
- Strategy development guides

Documentation contributions are reviewed with equal priority as code.

---

## 8. Licensing of Contributions

By submitting a contribution, you agree that:

- Your contribution is licensed under Apache License 2.0.
- You grant the project owner the right to use, modify, and redistribute your contribution.
- You confirm that the contribution is your original work or legally authorized.

---

## 9. Commercial Scope

Open-source contributions apply only to the public W2UOS codebase.

Private strategies, proprietary modules, enterprise extensions, and commercial deployments are not included in the open contribution scope.

---

## 10. Maintainer Review

All contributions are reviewed by the project maintainer.

The maintainer may request changes, clarification, or reject submissions that do not align with project goals, architecture, or long-term stability.

---

## 11. Contact

Author and Maintainer: pb_2n^ (孔穆清)  
GitHub: https://github.com/pb2n  
Project Repository: https://github.com/pb2n/W2UOS
