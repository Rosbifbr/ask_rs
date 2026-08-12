// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "AskApp",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "AskApp", targets: ["AskApp"]),
    ],
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", from: "1.5.1"),
    ],
    targets: [
        .executableTarget(
            name: "AskApp",
            dependencies: [
                .product(name: "SwiftTerm", package: "SwiftTerm"),
            ],
            path: "Sources/AskApp",
            swiftSettings: [
                .swiftLanguageMode(.v5),
            ]
        ),
    ]
)
